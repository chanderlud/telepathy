import 'package:telepathy/core/utils/io_shim.dart';
import 'package:flutter/material.dart';
import 'package:telepathy/widgets/chat/attachment_list.dart';
import 'package:telepathy/src/rust/flutter.dart';

class MessageItem extends StatelessWidget {
  final ChatMessage message;
  final bool isSender;
  final Map<String, (File?, Image?)> files;
  final void Function(Offset position, File? file) onShowAttachmentMenu;
  final void Function(Image image) onShowImagePreview;

  const MessageItem({
    super.key,
    required this.message,
    required this.isSender,
    required this.files,
    required this.onShowAttachmentMenu,
    required this.onShowImagePreview,
  });

  @override
  Widget build(BuildContext context) {
    final attachments = message.attachments();

    final children = <Widget>[
      if (message.text.isNotEmpty)
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          margin: const EdgeInsets.symmetric(vertical: 5),
          decoration: BoxDecoration(
            color: isSender
                ? Theme.of(context).colorScheme.secondary
                : Theme.of(context).colorScheme.tertiaryContainer,
            borderRadius: BorderRadius.only(
              topLeft: const Radius.circular(10.0),
              topRight: const Radius.circular(10.0),
              bottomLeft: Radius.circular(isSender ? 10.0 : 0),
              bottomRight: Radius.circular(isSender ? 0 : 10.0),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              Theme(
                data: ThemeData(
                  textSelectionTheme: TextSelectionThemeData(
                    selectionColor: isSender ? Colors.blue : null,
                  ),
                ),
                child: SelectableText(message.text),
              ),
              const SizedBox(width: 5),
              Text(
                message.time(),
                style: TextStyle(
                  fontSize: 10,
                  color: isSender ? Colors.white60 : Colors.grey,
                ),
              ),
            ],
          ),
        ),
      if (attachments.isNotEmpty)
        AttachmentList(
          attachments: attachments,
          files: files,
          onShowAttachmentMenu: onShowAttachmentMenu,
          onShowImagePreview: onShowImagePreview,
        ),
    ];

    return Align(
      alignment: isSender ? Alignment.centerRight : Alignment.centerLeft,
      child: Column(
        crossAxisAlignment:
            isSender ? CrossAxisAlignment.end : CrossAxisAlignment.start,
        children: children,
      ),
    );
  }
}
