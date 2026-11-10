# BLACK WOOF — Stage 3.1: local Tracks library

## Scope

This milestone adds a minimal local music catalog backed by Android MediaStore.
It does not add a Rust dependency, change the decoder/audio engine, or introduce
network access. The existing SAF `OPEN` action remains available as a fallback.

## Behavior

- The catalog is queried only after the user taps `LIBRARY` or `RESCAN`.
- Android 13+ requests `READ_MEDIA_AUDIO`; Android 9–12 use
  `READ_EXTERNAL_STORAGE` (manifest-limited to API 32).
- The query reads only MediaStore metadata: ID, title, artist, album and duration.
- No file contents are read during catalog discovery.
- A temporary worker thread performs the query; it exits when the query completes.
- Results are displayed as virtualized plain-text ListView rows on a black background.
- Selecting a row opens its content URI, detaches the file descriptor, and transfers
  ownership to the existing Rust `nativePlay` API.
- No polling, timer, artwork, thumbnails, internet permission, or background scan.

## Deliberate exclusions

Artist/album browsing, text search, queue navigation, shuffle/repeat, persisted
library snapshots, MediaSession, foreground service, and playback restoration are
not part of Stage 3.1. They require separate milestones and validation.

## Validation boundary

A successful CI build can verify compilation, APK contents, ABI, alignment, and
permission policy. It cannot prove that a particular device's MediaStore contains
all expected files or that every content provider supports descriptor detachment.
The owner must test permission grant/denial, catalog population, selection, playback,
rescan after adding/removing music, and manual SAF open on Android 15 / ARM64.
