import 'dart:io';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:telepathy/core/utils/index.dart';

/// Renders attachments for a [ChatMessage].
class AttachmentList extends StatelessWidget {
  final List<(String, Uint8List)> attachments;
  final Map<String, (File?, Image?)> files;
  final void Function(Offset position, File? file) onShowAttachmentMenu;
  final void Function(Image image) onShowImagePreview;

  const AttachmentList({
    super.key,
    required this.attachments,
    required this.files,
    required this.onShowAttachmentMenu,
    required this.onShowImagePreview,
  });

  @override
  Widget build(BuildContext context) {
    final widgets = attachments.map((attachment) {
      final file = files[attachment.$1];

      if (file == null) {
        DebugConsole.debug('Attachment file is null');
        return Text('Attachment: ${attachment.$1}');
      }

      // Image preview.
      if (file.$2 != null) {
        return Container(
          width: 500,
          margin: const EdgeInsets.symmetric(vertical: 5),
          child: InkWell(
            hoverColor: Colors.transparent,
            onTap: () => onShowImagePreview(file.$2!),
            onSecondaryTapDown: (details) =>
                onShowAttachmentMenu(details.globalPosition, file.$1),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(5.0),
              child: file.$2!,
            ),
          ),
        );
      }

      // Generic file.
      return InkWell(
        onSecondaryTapDown: (details) =>
            onShowAttachmentMenu(details.globalPosition, file.$1),
        child: Text('Attachment: ${attachment.$1}'),
      );
    }).toList();

    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: widgets,
    );
  }
}
