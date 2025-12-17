import 'dart:io';
import 'dart:typed_data';

import 'package:path_provider/path_provider.dart';
import 'package:telepathy/core/utils/console.dart';

// TODO verify cross-platform compatibility
Future<File?> saveToDownloads(Uint8List fileBytes, String fileName) async {
  Directory? downloadsDirectory = await getDownloadsDirectory();

  if (downloadsDirectory != null) {
    final subdirectory = Directory('${downloadsDirectory.path}/Telepathy');
    if (!await subdirectory.exists()) {
      await subdirectory.create();
    }

    final file = File('${subdirectory.path}/$fileName');
    await file.writeAsBytes(fileBytes);

    return file;
  } else {
    DebugConsole.warn('Unable to get downloads directory');
    return null;
  }
}

