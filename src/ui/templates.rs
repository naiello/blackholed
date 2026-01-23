use askama::Template;
use chrono::{DateTime, Utc};

use crate::api::model::{
    BlockEventResponse, ClientResponse, HostResponse, PaginatedResponse, SourceResponse,
};

pub struct GlobalPauseInfo {
    pub is_paused: bool,
    pub expires_at: Option<DateTime<Utc>>,
}

pub struct ClientPauseInfo {
    pub is_paused: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub ip: String,
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {
    pub global_pause: GlobalPauseInfo,
}

#[derive(Template)]
#[template(path = "events.html")]
pub struct EventsTemplate {
    pub current_ip: String,
    pub events: PaginatedResponse<BlockEventResponse>,
    pub global_pause: GlobalPauseInfo,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub current_ip: String,
    pub events: PaginatedResponse<BlockEventResponse>,
    pub global_pause: GlobalPauseInfo,
    pub client_pause: Option<ClientPauseInfo>,
}

#[derive(Template)]
#[template(path = "partials/event_list.html")]
pub struct EventListPartial {
    pub current_ip: String,
    pub events: PaginatedResponse<BlockEventResponse>,
}

#[derive(Template)]
#[template(path = "clients.html")]
pub struct ClientListTemplate {
    pub clients: PaginatedResponse<ClientResponse>,
    pub global_pause: GlobalPauseInfo,
}

#[derive(Template)]
#[template(path = "sources/list.html")]
pub struct SourceListTemplate {
    pub sources: PaginatedResponse<SourceResponse>,
}

#[derive(Template)]
#[template(path = "sources/new.html")]
pub struct NewSourceTemplate {
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "sources/detail.html")]
pub struct SourceDetailTemplate {
    pub source: SourceResponse,
    pub hosts: Option<PaginatedResponse<HostResponse>>,
}
