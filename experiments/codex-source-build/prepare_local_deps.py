#!/usr/bin/env python3
"""Replace unreachable Git dependencies with exact-revision local snapshots."""

from __future__ import annotations

from pathlib import Path


EXPERIMENT_DIR = Path(__file__).resolve().parent
LAB_ROOT = EXPERIMENT_DIR.parent.parent
WORKSPACE_MANIFEST = LAB_ROOT / "upstream/codex/codex-rs/Cargo.toml"
TCP_TUNNEL_MANIFEST = LAB_ROOT / "upstream/codex/codex-rs/tcp-tunnel/Cargo.toml"
LOCKFILE = LAB_ROOT / "upstream/codex/codex-rs/Cargo.lock"
TOKIO_TUNGSTENITE_MANIFEST = LAB_ROOT / "upstream/deps/tokio-tungstenite/Cargo.toml"


def replace(path: Path, old: str, new: str) -> None:
    text = path.read_text(encoding="utf-8")
    if new in text:
        return
    if old not in text:
        raise SystemExit(f"expected dependency declaration not found in {path}: {old}")
    path.write_text(text.replace(old, new), encoding="utf-8")


def main() -> None:
    replacements = [
        (
            'mxc-sdk = { git = "https://github.com/microsoft/mxc", rev = "b21120168d70befe12356411423266bfef168446" }',
            'mxc-sdk = { path = "../../deps/mxc/src/mxc-sdk" }',
        ),
        (
            'nucleo = { git = "https://github.com/helix-editor/nucleo.git", rev = "4253de9faabb4e5c6d81d946a5e35a90f87347ee" }',
            'nucleo = { path = "../../deps/nucleo" }',
        ),
        (
            'rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", rev = "98a54789b80110ad210495c4ad845c89e1980471", version = "=3.5.1", default-features = false }',
            'rmcp = { path = "../../deps/rust-sdk/crates/rmcp", version = "=3.5.1", default-features = false }',
        ),
        (
            'runfiles = { git = "https://github.com/dzbarsky/rules_rust", rev = "b56cbaa8465e74127f1ea216f813cd377295ad81" }',
            'runfiles = { path = "../../deps/rules_rust/rust/runfiles" }',
        ),
        (
            'crossterm = { git = "https://github.com/openai-oss-forks/crossterm", rev = "ed1cdab335221515706178d68495bba2aed1924f" }',
            'crossterm = { path = "../../deps/crossterm" }',
        ),
        (
            'tokio-tungstenite = { git = "https://github.com/openai-oss-forks/tokio-tungstenite", rev = "0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186" }',
            'tokio-tungstenite = { path = "../../deps/tokio-tungstenite" }',
        ),
        (
            'tungstenite = { git = "https://github.com/openai-oss-forks/tungstenite-rs", rev = "4fffad30fe373adbdcffab9545e9e9bf4f2fc19f" }',
            'tungstenite = { path = "../../deps/tungstenite-rs" }',
        ),
    ]
    for old, new in replacements:
        replace(WORKSPACE_MANIFEST, old, new)

    replace(
        TCP_TUNNEL_MANIFEST,
        'h3 = { git = "https://github.com/hyperium/h3", rev = "e07e69412876f7e26f026bd75a48b2704d8c8283" }',
        'h3 = { path = "../../../deps/h3/h3" }',
    )
    replace(
        TCP_TUNNEL_MANIFEST,
        'h3-quinn = { git = "https://github.com/hyperium/h3", rev = "e07e69412876f7e26f026bd75a48b2704d8c8283" }',
        'h3-quinn = { path = "../../../deps/h3/h3-quinn" }',
    )
    replace(
        TOKIO_TUNGSTENITE_MANIFEST,
        '[dependencies.tungstenite]\ngit = "https://github.com/openai-oss-forks/tungstenite-rs"\nrev = "4fffad30fe373adbdcffab9545e9e9bf4f2fc19f"\ndefault-features = false',
        '[dependencies.tungstenite]\npath = "../tungstenite-rs"\ndefault-features = false',
    )

    lock_text = LOCKFILE.read_text(encoding="utf-8")
    git_sources = [
        "git+https://github.com/openai-oss-forks/crossterm?rev=ed1cdab335221515706178d68495bba2aed1924f#ed1cdab335221515706178d68495bba2aed1924f",
        "git+https://github.com/hyperium/h3?rev=e07e69412876f7e26f026bd75a48b2704d8c8283#e07e69412876f7e26f026bd75a48b2704d8c8283",
        "git+https://github.com/microsoft/mxc?rev=b21120168d70befe12356411423266bfef168446#b21120168d70befe12356411423266bfef168446",
        "git+https://github.com/helix-editor/nucleo.git?rev=4253de9faabb4e5c6d81d946a5e35a90f87347ee#4253de9faabb4e5c6d81d946a5e35a90f87347ee",
        "git+https://github.com/modelcontextprotocol/rust-sdk?rev=98a54789b80110ad210495c4ad845c89e1980471#98a54789b80110ad210495c4ad845c89e1980471",
        "git+https://github.com/dzbarsky/rules_rust?rev=b56cbaa8465e74127f1ea216f813cd377295ad81#b56cbaa8465e74127f1ea216f813cd377295ad81",
        "git+https://github.com/openai-oss-forks/tokio-tungstenite?rev=0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186#0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186",
        "git+https://github.com/openai-oss-forks/tungstenite-rs?rev=4fffad30fe373adbdcffab9545e9e9bf4f2fc19f#4fffad30fe373adbdcffab9545e9e9bf4f2fc19f",
    ]
    for source in git_sources:
        lock_text = lock_text.replace(f'source = "{source}"\n', "")
    LOCKFILE.write_text(lock_text, encoding="utf-8")


if __name__ == "__main__":
    main()
