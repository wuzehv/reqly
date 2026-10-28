# Send Request Command Boundary Fix

## Problem

The Rust build fails because `send_request` is both annotated as a Tauri command and exposed to the sibling MCP module. Tauri's command macro generates helper macros for the function, and the restricted public visibility causes those generated names to be defined more than once.

## Design

Keep the Tauri command private and move the reusable HTTP request implementation into a crate-visible `execute_request` function. The private `send_request` command delegates directly to `execute_request`, while the MCP module calls `execute_request` itself.

This preserves one implementation of request construction, proxy handling, response decoding, and error reporting. It also keeps the Tauri command boundary separate from internal Rust APIs.

## Error Handling

Both call paths continue returning `Result<ResponseOutput, String>`, so existing frontend and MCP error behavior remains unchanged.

## Verification

Run `cargo check` for the Tauri crate and `npm.cmd run build` for the Vue frontend. If compilation exposes additional errors in the new MCP code, fix only issues required to restore a clean build.
