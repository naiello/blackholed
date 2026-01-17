use std::{collections::HashMap, io, iter, net::SocketAddr};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::{broadcast, RwLock};

use hickory_server::{
    authority::{
        Authority, LookupControlFlow, LookupError, LookupObject, LookupOptions, MessageRequest,
        UpdateResult, ZoneType,
    },
    proto::{
        op::ResponseCode,
        rr::{LowerName, Name, RecordType},
    },
    server::RequestInfo,
};

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct BlockEvent {
    pub time: DateTime<Utc>,
    pub src: SocketAddr,
    pub name: LowerName,
    pub record_type: RecordType,
}

pub struct BlocklistAuthority {
    origin: LowerName,
    blocklist: RwLock<HashMap<LowerName, BlocklistDisposition>>,
    wildcard_match: bool,
    min_wildcard_depth: u8,
    blocked_tx: broadcast::Sender<BlockEvent>,
}

pub enum BlocklistDisposition {
    Block,
    Allow,
}

impl BlocklistAuthority {
    pub fn new(origin: Name, config: &BlocklistConfig) -> Self {
        let (blocked_tx, _) = broadcast::channel(1024);
        Self {
            origin: origin.into(),
            blocklist: RwLock::new(HashMap::new()),
            wildcard_match: config.wildcard_match,
            min_wildcard_depth: config.min_wildcard_depth,
            blocked_tx,
        }
    }

    pub fn block_events(&self) -> broadcast::Receiver<BlockEvent> {
        self.blocked_tx.subscribe()
    }

    pub async fn load(&self, blocklist: HashMap<LowerName, BlocklistDisposition>) {
        *self.blocklist.write().await = blocklist
    }

    fn wildcards(&self, host: &Name) -> Vec<LowerName> {
        host.iter()
            .enumerate()
            .filter_map(|(i, _x)| {
                if i > ((self.min_wildcard_depth - 1) as usize) {
                    Some(host.trim_to(i + 1).into_wildcard().into())
                } else {
                    None
                }
            })
            .rev()
            .collect()
    }

    async fn is_blocked(&self, name: &LowerName) -> bool {
        let mut match_list = vec![name.to_owned()];

        if self.wildcard_match {
            match_list.append(&mut self.wildcards(name));
        }

        log::trace!("match_list: {:?}", match_list);
        let blocklist = self.blocklist.read().await;
        let disposition = match_list.iter().find_map(|entry| blocklist.get(entry));
        matches!(disposition, Some(BlocklistDisposition::Block))
    }
}

#[async_trait::async_trait]
impl Authority for BlocklistAuthority {
    type Lookup = BlocklistLookup;

    fn zone_type(&self) -> ZoneType {
        ZoneType::External
    }

    fn is_axfr_allowed(&self) -> bool {
        false
    }

    async fn update(&self, _update: &MessageRequest) -> UpdateResult<bool> {
        Err(ResponseCode::NotImp)
    }

    fn origin(&self) -> &LowerName {
        &self.origin
    }

    async fn lookup(
        &self,
        name: &LowerName,
        _rtype: RecordType,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        use LookupControlFlow::*;

        if !self.is_blocked(name).await {
            return Skip;
        }

        let nxdomain = LookupError::ResponseCode(ResponseCode::NXDomain);
        Break(Err(nxdomain))
    }

    async fn consult(
        &self,
        _name: &LowerName,
        _rtype: RecordType,
        _lookup_options: LookupOptions,
        last_result: LookupControlFlow<Box<dyn LookupObject>>,
    ) -> LookupControlFlow<Box<dyn LookupObject>> {
        last_result
    }

    async fn search(
        &self,
        request_info: RequestInfo<'_>,
        lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        let result = self
            .lookup(
                request_info.query.name(),
                request_info.query.query_type(),
                lookup_options,
            )
            .await;

        if matches!(result, LookupControlFlow::Break(_)) {
            let event = BlockEvent {
                time: Utc::now(),
                src: request_info.src.clone(),
                name: request_info.query.name().clone(),
                record_type: request_info.query.query_type(),
            };

            log::info!(
                "Blocked query for {} ({}) from {}",
                event.name,
                event.record_type,
                event.src,
            );

            self.blocked_tx
                .send(event)
                .inspect_err(|_| log::warn!("Failed to tx blocked event, no subscribers"))
                .ok();
        }

        result
    }

    async fn get_nsec_records(
        &self,
        _name: &LowerName,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<Self::Lookup> {
        LookupControlFlow::Continue(Err(LookupError::from(io::Error::other(
            "Blocklist cannot serve NSEC records",
        ))))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct BlocklistConfig {
    pub wildcard_match: bool,
    pub min_wildcard_depth: u8,
}

impl Default for BlocklistConfig {
    fn default() -> Self {
        Self {
            wildcard_match: true,
            min_wildcard_depth: 2,
        }
    }
}

pub struct BlocklistLookup {}

impl LookupObject for BlocklistLookup {
    fn is_empty(&self) -> bool {
        true
    }

    fn iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a hickory_server::proto::rr::Record> + Send + 'a> {
        Box::new(iter::empty())
    }

    fn take_additionals(&mut self) -> Option<Box<dyn LookupObject>> {
        None
    }
}

pub trait BlocklistLoader {}
