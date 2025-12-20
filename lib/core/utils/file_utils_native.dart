import 'dart:io';
import 'dart:typed_data';

import 'package:path_provider/path_provider.dart';
import 'package:telepathy/core/utils/console.dart';

/// Saves [fileBytes] to a Downloads location for the current platform.
///
/// Native behavior:
/// - Android: Attempts to save under the platform Downloads directory, in a
///   `Telepathy/` subfolder.
/// - iOS: Saves under the app-sandboxed downloads directory returned by
///   `path_provider`.
///
/// Returns the created [File], or `null` if the Downloads directory is
/// unavailable or an error occurs.
Future<File?> saveToDownloads(Uint8List fileBytes, String fileName) async {
  try {
    final Directory? downloadsDirectory = await getDownloadsDirectory();

    if (downloadsDirectory == null) {
      DebugConsole.warn('Unable to get downloads directory');
      return null;
    }

    final subdirectory = Directory('${downloadsDirectory.path}/Telepathy');
    try {
      if (!await subdirectory.exists()) {
        await subdirectory.create(recursive: true);
      }
    } catch (e) {
      DebugConsole.error('Failed to create downloads subdirectory: $e');
      return null;
    }

    final file = File('${subdirectory.path}/$fileName');
    try {
      await file.writeAsBytes(fileBytes);
    } catch (e) {
      DebugConsole.error('Failed to write file to downloads: $e');
      return null;
    }

    return file;
  } catch (e) {
    DebugConsole.error('saveToDownloads failed: $e');
    return null;
  }
}
