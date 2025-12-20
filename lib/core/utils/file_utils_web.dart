import 'dart:js_interop';
import 'dart:typed_data';

import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/core/utils/io_shim.dart';
import 'package:web/web.dart' as web;

/// Saves [fileBytes] to the user's Downloads folder (web).
///
/// Web behavior:
/// - Triggers a browser download using a Blob + HTMLAnchorElement.
/// - Returns `null` (web has no `dart:io` `File`).
///
/// Returns `null` on any error.
Future<File?> saveToDownloads(Uint8List fileBytes, String fileName) async {
  try {
    // Convert Dart bytes to a JS Uint8Array (Wasm-friendly).
    final blobParts = ([fileBytes.toJS] as dynamic) as JSArray<web.BlobPart>;

    final blob = web.Blob(
      blobParts,
      web.BlobPropertyBag(type: 'application/octet-stream'),
    );

    final url = web.URL.createObjectURL(blob);

    try {
      final anchor = web.HTMLAnchorElement()
        ..href = url
        ..download = fileName
        ..style.display = 'none';

      web.document.body?.append(anchor);
      anchor.click();
      anchor.remove();
    } finally {
      web.URL.revokeObjectURL(url);
    }

    return null;
  } catch (e) {
    DebugConsole.error('Web download failed: $e');
    return null;
  }
}
