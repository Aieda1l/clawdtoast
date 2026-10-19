Vendored softbuffer 0.4.8 (MIT OR Apache-2.0).
Only change: src/backends/cg.rs uses CGImageAlphaInfo::PremultipliedFirst instead of NoneSkipFirst,
so the macOS window honours the alpha channel. Drop this once upstream exposes an alpha mode.
