import 'dart:io';

import 'package:flutter/material.dart';
import 'package:process_run/process_run.dart';
import 'package:super_clipboard/super_clipboard.dart';
import 'package:telepathy/core/utils/console.dart';

/// A custom right click dialog.
class CustomPositionedDialog extends StatelessWidget {
  final Offset position;
  final File? file;

  const CustomPositionedDialog(
      {super.key, required this.position, required this.file});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () {
        Navigator.of(context).pop();
      },
      onSecondaryTap: () {
        Navigator.of(context).pop();
      },
      child: Stack(
        children: [
          Positioned(
            left: position.dx,
            top: position.dy,
            child: Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(5.0),
              ),
              padding: const EdgeInsets.all(10),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  InkWell(
                    onTap: () async {
                      final clipboard = SystemClipboard.instance;

                      if (clipboard == null) {
                        DebugConsole.warn(
                            'Clipboard not supported on this platform');
                      } else {
                        final item = DataWriterItem();

                        if (file != null) {
                          item.add(Formats.fileUri(Uri(path: file!.path)));
                        } else {
                          DebugConsole.warn('File is null');
                        }

                        clipboard.write([item]);
                      }

                      if (context.mounted) {
                        Navigator.of(context).pop();
                      }
                    },
                    child: const SizedBox(
                      width: 125,
                      child: Text('Copy'),
                    ),
                  ),
                  // TODO need some kind of divider here
                  const SizedBox(height: 5),
                  if ((Platform.isMacOS ||
                          Platform.isLinux ||
                          Platform.isWindows) &&
                      file != null)
                    InkWell(
                      onTap: () {
                        // init shell
                        Shell shell = Shell();

                        // TODO work on cross platform support
                        if (Platform.isWindows) {
                          shell.run(
                              'explorer.exe /select,${file!.path.replaceAll("/", "\\\\")}');
                        } else if (Platform.isMacOS) {
                          shell.run('open -R "${file!.path}"');
                        } else {
                          DebugConsole.warn(
                              'Opening file in folder not supported on this platform');
                        }

                        Navigator.of(context).pop();
                      },
                      child: const SizedBox(
                        width: 125,
                        child: Text('View in Folder'),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
