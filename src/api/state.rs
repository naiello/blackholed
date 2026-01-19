use std::sync::Arc;

use crate::{
    blocklist::{BlocklistAuthority, BlocklistProvider},
    db::Db,
    eventstore::EventStore,
};

/// Shared state for API handlers
pub struct ApiState<DB, BP, ES>
where
    DB: Db,
    BP: BlocklistProvider,
    ES: EventStore,
{
    pub db: Arc<DB>,
    pub blocklist: Arc<BlocklistAuthority<BP>>,
    pub eventstore: Arc<ES>,
}

impl<DB, BP, ES> Clone for ApiState<DB, BP, ES>
where
    DB: Db,
    BP: BlocklistProvider,
    ES: EventStore,
{
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            blocklist: self.blocklist.clone(),
            eventstore: self.eventstore.clone(),
        }
    }
}

impl<DB, BP, ES> ApiState<DB, BP, ES>
where
    DB: Db,
    BP: BlocklistProvider,
    ES: EventStore,
{
    pub fn new(db: Arc<DB>, blocklist: Arc<BlocklistAuthority<BP>>, eventstore: Arc<ES>) -> Self {
        Self {
            db,
            blocklist,
            eventstore,
        }
    }
}
