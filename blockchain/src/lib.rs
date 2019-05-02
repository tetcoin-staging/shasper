use beacon::{H256, KeccakHasher, BeaconState, BeaconBlock, Error as BeaconError, NoVerificationConfig, Inherent, Transaction, UnsealedBeaconBlock};
use blockchain::traits::{Block as BlockT, BlockExecutor, BuilderExecutor};
use ssz::Hashable;

#[derive(Eq, PartialEq, Clone)]
pub struct Block(BeaconBlock);

impl BlockT for Block {
	type Identifier = H256;

	fn id(&self) -> H256 {
		Hashable::<KeccakHasher>::hash(&self.0)
	}

	fn parent_id(&self) -> Option<H256> {
		if self.0.previous_block_root == H256::default() {
			None
		} else {
			Some(self.0.previous_block_root)
		}
	}
}

pub trait StateExternalities {
	fn state(&mut self) -> &mut BeaconState;
}

pub struct State {
	state: BeaconState,
}

impl StateExternalities for State {
	fn state(&mut self) -> &mut BeaconState {
		&mut self.state
	}
}

#[derive(Debug)]
pub enum Error {
	Backend(Box<std::error::Error>),
	Beacon(BeaconError),
}

impl std::fmt::Display for Error {
	fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
		write!(f, "{:?}", self)
	}
}

impl std::error::Error for Error { }

pub struct Executor;

impl BlockExecutor for Executor {
	type Error = Error;
	type Block = Block;
	type Externalities = dyn StateExternalities + 'static;

	fn execute_block(
		&self,
		block: &Block,
		state: &mut Self::Externalities,
	) -> Result<(), Error> {
		let config = NoVerificationConfig::full();

		beacon::execute_block(&block.0, state.state(), &config)
			.map_err(|e| Error::Beacon(e))
	}
}

impl BuilderExecutor for Executor {
	type Error = Error;
	type Block = Block;
	type BuildBlock = UnsealedBeaconBlock;
	type Externalities = dyn StateExternalities + 'static;
	type Inherent = Inherent;
	type Extrinsic = Transaction;

	fn initialize_block(
		&self,
		parent_block: &Block,
		state: &mut Self::Externalities,
		inherent: Inherent,
	) -> Result<UnsealedBeaconBlock, Self::Error> {
		let config = NoVerificationConfig::full();

		beacon::initialize_block(&parent_block.0, state.state(), inherent, &config)
			.map_err(|e| Error::Beacon(e))
	}

	fn apply_extrinsic(
		&self,
		block: &mut UnsealedBeaconBlock,
		extrinsic: Transaction,
		state: &mut Self::Externalities,
	) -> Result<(), Self::Error> {
		let config = NoVerificationConfig::full();

		beacon::apply_transaction(block, state.state(), extrinsic, &config)
			.map_err(|e| Error::Beacon(e))
	}

	fn finalize_block(
		&self,
		block: &mut UnsealedBeaconBlock,
		state: &mut Self::Externalities,
	) -> Result<(), Self::Error> {
		let config = NoVerificationConfig::full();

		beacon::finalize_block(block, state.state(), &config)
			.map_err(|e| Error::Beacon(e))
	}
}