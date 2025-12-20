/// Cross-platform file saving helpers.
///
/// This library uses conditional exports to provide a platform-specific
/// implementation of [saveToDownloads] while keeping a stable import path.
///
/// Platform behavior:
/// - Android: Saves to the platform Downloads directory, under `Telepathy/`.
/// - iOS: Saves to an app-sandboxed downloads directory (via `path_provider`).
/// - Web: Triggers a browser download and returns `null`.
///
/// Return value:
/// - Native: Returns a `dart:io` `File` on success.
/// - Web: Returns `null` (no `dart:io` `File`), and also returns `null` on error.
library;

export 'file_utils_web.dart' if (dart.library.io) 'file_utils_native.dart';
