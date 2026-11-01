# Recording Safety Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement task-by-task.

**Goal:** Prevent premature exit from corrupting recordings and provide MKV recovery with configurable retention.
**Architecture:** Typed lifecycle/events, GStreamer MKV capture, FFmpeg atomic remux, persistent preferences and rescue UI.
**Tech Stack:** Rust, egui/eframe 0.27, GStreamer 0.22, FFmpeg/ffprobe, serde, rfd, fs2.
**Spec:** docs/superpowers/specs/2026-10-31-recording-safety-design.md

## Constraints and review focus
- Keep original and failed MKV files; delete only after verified MP4 publication and explicit saved retention preference.
- Never overwrite existing MP4; handle Unicode/space paths without shell interpolation.
- Closing during preparation, saving, or conversion waits for worker termination.
- Failed/truncated MKV conversion and insufficient disk space report failure and preserve source.
- Saved MKV files with completed MP4 are excluded from automatic recovery suggestions.

## Tasks
- [x] Lifecycle: first test stop/close/late events/failure, implement transitions; run rustc tests.
- [x] Storage/conversion: first test malformed input preservation, output collisions, keep/delete preferences, truncated MKV recovery and settings roundtrip; implement real FFmpeg conversion and durable config.
- [x] Capture/UI: switch muxer, enforce EOS, retain and join worker, close interception, cancellation during portal, disk monitor, recovery selector and retention setting.
- [x] Verification: full cargo test, fmt/clippy/build, real GStreamer normal/interrupted recording conversion and UI close integration; document dependencies/limitations; final review.

Execution in current workspace: repository has no commits and all project files are untracked, so a worktree would omit the existing source. No automatic baseline commit of user files or media.

## Execution / verification record

- Lifecycle RED: four tests failed for premature idle/exit, late start events and false success; GREEN: all pass.
- Conversion/settings RED: conversion, retention, collision, recovery scan and persistence tests failed; GREEN: all pass with real FFmpeg.
- Final independent review found queued-completion race, pre-capture cancellation treated as failure, omitted-track deletion risk and missing progress for durationless MKV. Each was reproduced by a regression test (four RED failures) and corrected; all now pass.
- Verified full suite: 20 tests passed, including real GStreamer A/V EOS finalization and SIGKILL recording recovery. Full FFmpeg decode passed for generated test outputs.
- Actual native GUI (Xvfb) stop-then-close and close-while-recording passed with retention disabled/enabled respectively; does not record user desktop.
- rustfmt check passed. Clippy --all-targets -- -D warnings passed. Release build passed; existing desktop entry already points to target/release/capra.
- No user videos/configuration were modified by tests. No commits were created because the entire project and media were already untracked in an unborn repository.
- Decisions: retain MKV by default for safety; reject manual sources with additional video/subtitle/attachment tracks so the deletion option cannot discard omitted content; keep window open on genuine conversion errors so the user can see the failure and retry. Closing a pending portal before capture is normal cancellation and exits as requested.
- Limits: no one-hour real screen recording, real power-loss test, or interactive Wayland portal authorization was performed. X11 GUI and GStreamer process-kill recovery were exercised.
