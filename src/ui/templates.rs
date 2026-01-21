use askama::Template;

use crate::api::model::{
    BlockEventResponse, ClientResponse, HostResponse, PaginatedResponse, SourceResponse,
};

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTemplate {}

#[derive(Template)]
#[template(path = "events.html")]
pub struct EventsTemplate {
    pub current_ip: String,
    pub events: PaginatedResponse<BlockEventResponse>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub current_ip: String,
    pub events: PaginatedResponse<BlockEventResponse>,
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
