# Tauri + Vue 3

This template should help get you started developing with Tauri + Vue 3 in Vite. The template uses Vue 3 `<script setup>` SFCs, check out the [script setup docs](https://v3.vuejs.org/api/sfc-script-setup.html#sfc-script-setup) to learn more.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Vue - Official](https://marketplace.visualstudio.com/items?itemName=Vue.volar) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)

### Linux build prerequisites

Tauri requires native GTK/WebKit development packages. On Ubuntu 24.04 or WSL Ubuntu, install them before running `cargo check` or `npm run tauri -- dev`:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

## Local MCP Server

Reqly starts a local, stateless MCP Streamable HTTP server with the desktop application:

```text
http://127.0.0.1:3765/mcp
```

Generic MCP client configuration:

```json
{
  "mcpServers": {
    "reqly": {
      "type": "streamable-http",
      "url": "http://127.0.0.1:3765/mcp"
    }
  }
}
```

The server listens on `0.0.0.0`, so local clients can use `127.0.0.1` and LAN clients can use the machine's LAN IP, for example `http://192.168.1.20:3765/mcp`. The exact outer configuration key can vary by client, but the endpoint and MCP protocol are the same. Reqly must be running while a client uses the server. The MCP service can be enabled, disabled, or moved to another port from Reqly's global settings.

Available tools:

- `reqly_list_collections`: list collections and saved requests
- `reqly_create_collection`: create a collection
- `reqly_create_subgroup`: create a second-level group that inherits its parent collection Pre-request script
- `reqly_create_request`: create a request in a collection or subgroup with Query parameters, headers, and JSON/Text/Form body
- `reqly_run_request`: run a saved request with optional workspace-variable overrides

The MCP server is accessible on the local network and does not currently require authentication. Use a trusted network and restrict access with the system firewall. Browser origins other than localhost are rejected. Saved requests run through Reqly's normal execution pipeline, including workspace variables, Pre-request scripts, Post-response scripts, retries, and supported body types.

## Local Storage

The desktop application stores data in `reqly.db`, an embedded SQLite database in the Tauri application data directory. On the first upgraded launch, an existing `workspace.json` is imported in a transaction and copied to `workspace.pre-sqlite-backup.json`; the original JSON file is not deleted.

The UI loads collection and request summaries at startup and fetches full request details only when a request is selected. Browser-only development mode continues to use the `reqly-workspace` localStorage entry.
