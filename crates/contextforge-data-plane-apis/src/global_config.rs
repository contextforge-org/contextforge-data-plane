use http::uri::Authority;
use url::Url;
#[derive(Debug, Clone, Default)]
pub struct GlobalConfig {
    /// Maximum number of MCP standard headers accepted on a single request.
    pub mcp_standard_header_max_count: Option<usize>,

    /// Maximum byte length accepted for a single MCP standard header value.
    pub mcp_standard_header_max_value_bytes: Option<usize>,

    /// Approximate request-level aggregate bytes accepted across all matched
    /// MCP standard header names and values.
    pub mcp_standard_header_max_total_bytes: Option<usize>,
    pub mcp_allowed_origins: Option<Vec<Url>>,
    pub mcp_allowed_hosts: Option<Vec<Authority>>,
}
