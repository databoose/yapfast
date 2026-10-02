$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

function Invoke-Cargo {
    & cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo failed with exit code $LASTEXITCODE" }
}

Invoke-Cargo fmt --all -- --check
Invoke-Cargo clippy --locked --workspace --all-targets -- -D warnings
Invoke-Cargo test --locked --workspace
Invoke-Cargo build --locked --workspace
Invoke-Cargo run --locked -- --smoke-test
Invoke-Cargo run --locked --no-default-features -- --smoke-test
