initSidebarItems({"constant":[["CONFIRMATION_DEPTH_K",""],["DECIMAL_PLACES","Subspace Credits have 18 decimal places."],["MAX_PLOT_SIZE","Maximum number of pieces in each plot"],["MIN_REPLICATION_FACTOR","Minimum desired number of replicas of the blockchain to be stored by the network, impacts storage fees."],["RECORDED_HISTORY_SEGMENT_SIZE","Recorded History Segment Size includes half of the records (just data records) that will later be erasure coded and together with corresponding witnesses will result in `MERKLE_NUM_LEAVES` pieces of archival history."],["RECORD_SIZE","Size of a segment record given the global piece size (in bytes)."],["SHANNON","The smallest unit of the token is called Shannon."],["SSC","One Subspace Credit."],["STORAGE_FEES_ESCROW_BLOCK_REWARD","How much (ratio) of storage fees escrow should be given to farmer each block as a reward."],["STORAGE_FEES_ESCROW_BLOCK_TAX","How much (ratio) of storage fees collected in a block should be put into storage fees escrow (with remaining issued to farmer immediately)."]],"mod":[["opaque","Opaque types. These are used by the CLI to instantiate machinery that don’t need to know the specifics of the runtime. They can then be made to be agnostic over specific formats of data like extrinsics, allowing for them to continue syncing the network through upgrades to even the core data structures."]],"trait":[["FindBlockRewardAddress","A trait for finding the address for a block reward based on the `PreRuntime` digests contained within it."]],"type":[["AccountId","Some way of identifying an account on the chain. We intentionally make it equivalent to the public key of our transaction signing scheme."],["Balance","Balance of an account."],["BlockNumber","Block number in Subspace network."],["Hash","A hash of some data used by the chain."],["Index","Index of a transaction in the chain."],["Moment","Type used for expressing timestamp."],["Signature","Alias to 512-bit hash when used in the context of a transaction signature on the chain."]]});