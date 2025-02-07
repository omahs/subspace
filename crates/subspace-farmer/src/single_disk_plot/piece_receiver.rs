use crate::NodeClient;
use async_trait::async_trait;
use backoff::future::retry;
use backoff::ExponentialBackoff;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use lru::LruCache;
use parity_scale_codec::Decode;
use parking_lot::Mutex;
use std::collections::BTreeSet;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use subspace_archiving::archiver::is_piece_valid;
use subspace_core_primitives::crypto::kzg::Kzg;
use subspace_core_primitives::{
    Piece, PieceIndex, PieceIndexHash, RecordsRoot, SegmentIndex, PIECES_IN_SEGMENT, RECORD_SIZE,
};
use subspace_farmer_components::plotting::PieceReceiver;
use subspace_networking::libp2p::PeerId;
use subspace_networking::utils::multihash::MultihashCode;
use subspace_networking::{Node, PieceByHashRequest, PieceKey, ToMultihash};
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, trace, warn};

/// Defines initial duration between get_piece calls.
const GET_PIECE_INITIAL_INTERVAL: Duration = Duration::from_secs(1);
/// Defines max duration between get_piece calls.
const GET_PIECE_MAX_INTERVAL: Duration = Duration::from_secs(5);
/// Delay for getting piece from cache before resorting to archival storage
const GET_PIECE_ARCHIVAL_STORAGE_DELAY: Duration = Duration::from_secs(2);
/// Max time allocated for getting piece from DSN before attempt is considered to fail
const GET_PIECE_TIMEOUT: Duration = Duration::from_secs(5);

// Temporary struct serving pieces from different providers using configuration arguments.
pub(crate) struct MultiChannelPieceReceiver<'a, NC> {
    dsn_node: &'a Node,
    node_client: &'a NC,
    kzg: &'a Kzg,
    records_root_cache: &'a Mutex<LruCache<SegmentIndex, RecordsRoot>>,
    cancelled: &'a AtomicBool,
}

impl<'a, NC> MultiChannelPieceReceiver<'a, NC>
where
    NC: NodeClient,
{
    pub(crate) fn new(
        dsn_node: &'a Node,
        node_client: &'a NC,
        kzg: &'a Kzg,
        records_root_cache: &'a Mutex<LruCache<SegmentIndex, RecordsRoot>>,
        cancelled: &'a AtomicBool,
    ) -> Self {
        Self {
            dsn_node,
            node_client,
            kzg,
            records_root_cache,
            cancelled,
        }
    }

    fn check_cancellation(&self) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
        if self.cancelled.load(Ordering::Acquire) {
            debug!("Getting a piece was cancelled.");

            return Err("Getting a piece was cancelled.".into());
        }

        Ok(())
    }

    // restore after fixing https://github.com/libp2p/rust-libp2p/issues/3048
    // Get from piece cache (L2) using providers
    #[allow(dead_code)]
    async fn get_piece_from_cache_by_providers(&self, _piece_index: PieceIndex) -> Option<Piece> {
        None

        // TODO: uncomment on fixing https://github.com/libp2p/rust-libp2p/issues/3048
        // let providers_result = dsn_node.get_providers(key).await;
        //
        // info!(?key, "get_providers result: {:?}", providers_result);
        //
        // for provider in providers_result? {
        //     let response_result = dsn_node
        //         .send_generic_request(
        //             provider,
        //             PieceByHashRequest {
        //                 key: PieceKey::PieceIndex(piece_index),
        //             },
        //         )
        //         .await;
        //
        //     info!(
        //         ?key,
        //         "send_generic_request for PieceByHashRequest result: {:?}", response_result
        //     );
        //
        //     if let Some(piece) = response_result?.piece {
        //         return Ok(Some(piece));
        //     }
        // }
    }

    // Get from piece cache (L2)
    async fn get_piece_from_cache(&self, piece_index: PieceIndex) -> Option<Piece> {
        let key = PieceIndexHash::from_index(piece_index).to_multihash();

        let get_value_result = self.dsn_node.get_value(key).await;

        let mut get_value_stream = match get_value_result {
            Ok(get_value_stream) => get_value_stream,
            Err(err) => {
                error!(%piece_index, ?key, ?err, "get_value returned an error");
                return None;
            }
        };

        let piece_record = match get_value_stream.next().await {
            Some(piece_record) => {
                trace!(%piece_index, ?key, "get_value returned a piece");

                piece_record
            }
            None => {
                debug!(%piece_index, ?key, "get_value returned no piece");
                return None;
            }
        };

        let piece: Piece = match piece_record.record.value.try_into() {
            Ok(piece) => piece,
            Err(error) => {
                error!(%piece_index, ?key, ?error, "Error on piece construction");
                return None;
            }
        };

        if let Some(source_peer_id) = piece_record.peer && source_peer_id != self.dsn_node.id() {
            let segment_index: SegmentIndex = piece_index / PieceIndex::from(PIECES_IN_SEGMENT);

            let maybe_records_root = self.records_root_cache.lock().get(&segment_index).copied();
            let records_root = match maybe_records_root {
                Some(records_root) => records_root,
                None => {
                    let records_roots =
                        match self.node_client.records_roots(vec![segment_index]).await {
                            Ok(records_roots) => records_roots,
                            Err(error) => {
                                error!(
                                    %piece_index,
                                    ?key,
                                    ?error,
                                    "Failed tor retrieve records root from node"
                                );
                                return None;
                            }
                        };

                    let records_root = match records_roots.into_iter().next().flatten() {
                        Some(records_root) => records_root,
                        None => {
                            error!(
                                %piece_index,
                                %segment_index,
                                ?key,
                                "Records root for segment index wasn't found on node"
                            );
                            return None;
                        }
                    };

                    self.records_root_cache
                        .lock()
                        .push(segment_index, records_root);

                    records_root
                }
            };

            if !is_piece_valid(
                self.kzg,
                PIECES_IN_SEGMENT,
                &piece,
                records_root,
                u32::try_from(piece_index % PieceIndex::from(PIECES_IN_SEGMENT))
                    .expect("Always fix into u32; qed"),
                RECORD_SIZE,
            ) {
                error!(
                    %piece_index,
                    %source_peer_id,
                    ?key,
                    "Received invalid piece from peer"
                );

                // We don't care about result here
                let _ = self.dsn_node.ban_peer(source_peer_id).await;
                return None;
            }
        }

        Some(piece)
    }

    // Get piece from archival storage (L1) from sectors. Log errors.
    async fn get_piece_from_archival_storage(&self, piece_index: PieceIndex) -> Option<Piece> {
        let key =
            PieceIndexHash::from_index(piece_index).to_multihash_by_code(MultihashCode::Sector);

        let get_value_result = self.dsn_node.get_value(key).await;

        match get_value_result {
            Ok(mut encoded_gset_stream) => {
                match encoded_gset_stream.next().await {
                    Some(encoded_gset_record) => {
                        trace!(
                            %piece_index,
                            ?key,
                            "get_value returned a piece-by-sector providers"
                        );

                        // Workaround for archival sector until we fix https://github.com/libp2p/rust-libp2p/issues/3048
                        let peer_set = if let Ok(set) = BTreeSet::<Vec<u8>>::decode(
                            &mut encoded_gset_record.record.value.as_slice(),
                        ) {
                            set
                        } else {
                            warn!(
                                %piece_index,
                                ?key,
                                "get_value returned a non-gset value"
                            );
                            return None;
                        };

                        for peer_id in peer_set.into_iter() {
                            if let Ok(piece_provider_id) = PeerId::from_bytes(&peer_id) {
                                let request_result = self
                                    .dsn_node
                                    .send_generic_request(
                                        piece_provider_id,
                                        PieceByHashRequest {
                                            key: PieceKey::Sector(PieceIndexHash::from_index(
                                                piece_index,
                                            )),
                                        },
                                    )
                                    .await;

                                match request_result {
                                    Ok(request) => {
                                        if let Some(piece) = request.piece {
                                            return Some(piece);
                                        }
                                    }
                                    Err(error) => {
                                        error!(
                                            %piece_index,
                                            ?peer_id,
                                            ?key,
                                            ?error,
                                            "Error on piece-by-hash request."
                                        );
                                    }
                                }
                            } else {
                                error!(
                                    %piece_index,
                                    ?peer_id,
                                    ?key,
                                    "Cannot convert piece-by-sector provider PeerId from received \
                                    bytes"
                                );
                            }
                        }
                    }
                    None => {
                        info!(%piece_index,?key, "get_value returned no piece-by-sector provider");
                    }
                }
            }
            Err(err) => {
                error!(%piece_index,?key, ?err, "get_value returned an error (piece-by-sector)");
            }
        }

        None
    }
}

#[async_trait]
impl<'a, NC> PieceReceiver for MultiChannelPieceReceiver<'a, NC>
where
    NC: NodeClient,
{
    async fn get_piece(
        &self,
        piece_index: PieceIndex,
    ) -> Result<Option<Piece>, Box<dyn Error + Send + Sync + 'static>> {
        trace!(%piece_index, "Piece request.");

        let backoff = ExponentialBackoff {
            initial_interval: GET_PIECE_INITIAL_INTERVAL,
            max_interval: GET_PIECE_MAX_INTERVAL,
            // Try until we get a valid piece
            max_elapsed_time: None,
            ..ExponentialBackoff::default()
        };

        retry(backoff, || async {
            self.check_cancellation()
                .map_err(backoff::Error::Permanent)?;

            // Try to pull pieces in two ways, whichever is faster
            let mut piece_attempts = [
                timeout(
                    GET_PIECE_TIMEOUT,
                    Box::pin(self.get_piece_from_cache(piece_index))
                        as Pin<Box<dyn Future<Output = _> + Send>>,
                ),
                //TODO: verify "broken pipe" error cause
                timeout(
                    GET_PIECE_TIMEOUT,
                    Box::pin(async {
                        // Prefer cache if it can return quickly, otherwise fall back to archival storage
                        sleep(GET_PIECE_ARCHIVAL_STORAGE_DELAY).await;
                        self.get_piece_from_archival_storage(piece_index).await
                    }) as Pin<Box<dyn Future<Output = _> + Send>>,
                ),
            ]
            .into_iter()
            .collect::<FuturesUnordered<_>>();

            while let Some(maybe_piece) = piece_attempts.next().await {
                if let Ok(Some(piece)) = maybe_piece {
                    trace!(%piece_index, "Got piece");
                    return Ok(Some(piece));
                }
            }

            warn!(%piece_index, "Couldn't get a piece from DSN. Retrying...");

            Err(backoff::Error::transient(
                "Couldn't get piece from DSN".into(),
            ))
        })
        .await
    }
}
