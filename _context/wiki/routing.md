# MCP Routing Semantics

The external dataplane is a **pure stateless router**. No session state, no `BackendTransports`, no sticky-routing requirement.

## How a request is routed

1. `validate_stateless` extracts `VirtualHost` from request extensions (set by `virtual_host_config` layer from the JWT virtual-host ID).
2. Downstream name is looked up in `VirtualHost::tools`, `::resources`, or `::prompts` — an O(1) table lookup.
3. `connect_backend_for_request` opens a fresh `StreamableHttpClientTransport`, runs the call, closes the connection.

The control plane builds and publishes the routing tables to Redis; the dataplane never derives names at call time.

## Routing table shape

```text
VirtualHost { backends: HashMap<String, BackendMCPGateway>,
              tools: HashMap<String, ServiceRoute>,
              resources: HashMap<String, ServiceRoute>,
              resource_templates: HashMap<String, ServiceRoute>,
              prompts: HashMap<String, ServiceRoute> }

ServiceRoute { backend_name: String,   // key into VirtualHost::backends
               upstream_name: String } // name/URI forwarded to the backend
```

Source: [`user_store.rs`](../../crates/contextforge-data-plane-apis/src/user_store.rs)

## Method quick reference

| Method | Behavior |
| --- | --- |
| `initialize` (`2026-07-28`) | `INVALID_REQUEST` — not supported by this dataplane. |
| `initialize` (legacy) | Stub `InitializeResult`; no backend fanout. Supports older clients during migration. |
| `list_tools`, `list_resources`, `list_resource_templates`, `list_prompts` | `INVALID_REQUEST` — delegated to control plane. |
| `call_tool` | Lookup in `tools` map → pre-hook → fresh connection → call → post-hook → close. Forwards cancellation; tracks progress tokens. |
| `read_resource` | Lookup in `resources` map → fresh connection → call with upstream URI → close. |
| `get_prompt` | Lookup in `prompts` map → pre-hook → fresh connection → call → post-hook → close. |
| `subscribe`, `unsubscribe`, `complete` | `INVALID_REQUEST` — delegated to control plane. |
| `ping` | Local success; no backend fanout. |
| `DELETE` | RMCP handles; `session_id_layer` removes the `LocalUserSessionStore` entry. No backend state to clean up. |

## Header forwarding

Applied in order per upstream call: Host (from backend URL, HTTPS only) → passthrough (`BackendMCPGateway::passthrough_headers`) → `Mcp-Param-*` auto-forward → trace context → add (`add_headers`, overrides passthrough) → remove (`remove_headers`, applied last).

Protected headers that config can never touch: `Host`, `Content-Length`, `Content-Type`, all RFC 7230 hop-by-hop headers, `Mcp-Session-Id`, `Accept`, `Last-Event-Id`, and all computed MCP standard headers (`Mcp-Method`, `Mcp-Name`, `Mcp-Protocol-Version`, `Mcp-Param-*`).

For clients on `≥ 2026-07-28`, `call_tool` validates `Mcp-Param-*` headers against `BackendMCPGateway::tool_schemas` before contacting the backend.

## Plugin hooks

`call_tool`, `get_prompt`, and `read_resource` run pre/post hooks when a
`GatewayPluginRuntimeHandle` is configured. Pre-hooks can deny a request or edit
tool/prompt arguments or the resource URI. Resource URI edits must resolve through
the caller's published routes. Post-hooks can rewrite or reject the response.

The handle selects a runtime before backend I/O. It returns typed request state
whose `after_*` method runs the post-hook on that same runtime, even after a reload
or a reload failure. A request that started without post-hooks never gains one
mid-flight. Tool state is shared under a mutex so progress notifications and the
final response use the same correlation ID and serialize plugin-context updates.
Prompt and resource state is owned by a single request and needs no mutex.

The internal CPEX crate separates these responsibilities:

| Module | Owns |
| --- | --- |
| `registry/` | Runtime selection, factory registration, config reloads, and the watcher |
| `runtime.rs` | Manager lifecycle, shared pre/post execution, and request context |
| `tools/`, `prompts/`, `resources/` | Typed request state, operation-specific CMF conversion, and conversion tests |
| `cmf.rs` | Hook inventory, message envelopes, and the `CmfResponse` conversion trait |
| `hooks.rs` | Shared argument edits and pre-hook results |
| `config.rs`, `factory.rs` | Config decoding/storage and compiled-in plugin factories |

Each response adapter implements `CmfResponse`; the runner handles invocation,
unchanged payloads, context propagation, and denial. Conversion and rejection
rules remain operation-specific: tools, rendered prompts, and resource reads
have different MCP representations. See [Config](config.md#tool-call-hook-behavior)
for those contracts.
