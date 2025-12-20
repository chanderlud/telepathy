import 'package:telepathy/core/utils/io_shim_stub.dart';

import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/chat/message_item.dart';
import 'package:telepathy/widgets/chat/selected_attachments.dart';

class ChatWidget extends StatefulWidget {
  final Telepathy telepathy;
  final StateController stateController;
  final ProfilesController profilesController;
  final ChatStateController chatStateController;
  final SoundPlayer player;

  const ChatWidget(
      {super.key,
      required this.telepathy,
      required this.stateController,
      required this.chatStateController,
      required this.player,
      required this.profilesController});

  @override
  State<StatefulWidget> createState() => ChatWidgetState();
}

class ChatWidgetState extends State<ChatWidget> {
  static const OutlineInputBorder _noBorder = OutlineInputBorder(
    borderSide: BorderSide(color: Colors.transparent),
  );

  final FocusNode _focusNode = FocusNode();
  late final ChatInputController _chatInputController;

  @override
  void initState() {
    super.initState();
    widget.stateController.addListener(_onStateControllerChange);

    _chatInputController = ChatInputController(
      chatStateController: widget.chatStateController,
      focusNode: _focusNode,
    )..init();

    // NOTE: keyboard focus + paste handling are managed by [_chatInputController].
  }

  @override
  void dispose() {
    widget.stateController.removeListener(_onStateControllerChange);
    _chatInputController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void sendMessage(String text) async {
    if (!widget.chatStateController.active) return;
    if (text.isEmpty && widget.chatStateController.attachments.isEmpty) return;

    final Contact? contact = widget.stateController.activeContact;
    // Chat is only supported for direct contact calls right now (not rooms).
    if (contact == null) return;

    try {
      ChatMessage message = widget.telepathy.buildChat(
          contact: contact,
          text: text,
          attachments: widget.chatStateController.attachments);
      await widget.telepathy.sendChat(message: message);

      message.clearAttachments();
      widget.chatStateController.messages.add(message);
      widget.chatStateController.clearInput();
    } on DartError catch (error) {
      if (!mounted) return;
      showErrorDialog(context, 'Message Send Failed', error.message);
    }
  }

  void _onStateControllerChange() {
    // Only enable chat when a direct contact is active.
    final bool desiredActive = widget.stateController.activeContact != null;

    if (desiredActive == widget.chatStateController.active) {
      return;
    } else if (!desiredActive && widget.chatStateController.active) {
      widget.chatStateController.clearState();
    }

    setState(() {
      widget.chatStateController.active = desiredActive;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Flexible(
            fit: FlexFit.loose,
            child: ListenableBuilder(
              listenable: widget.chatStateController,
              builder: (BuildContext context, Widget? child) {
                return ListView.builder(
                    itemCount: widget.chatStateController.messages.length,
                    itemBuilder: (BuildContext context, int index) {
                      ChatMessage message =
                          widget.chatStateController.messages[index];
                      bool sender = message.isSender(
                          identity: widget.profilesController.peerId);

                      return MessageItem(
                        message: message,
                        isSender: sender,
                        files: widget.chatStateController.files,
                        onShowAttachmentMenu: showAttachmentMenu,
                        onShowImagePreview: showImagePreview,
                      );
                    });
              },
            )),
        ListenableBuilder(
            listenable: widget.stateController,
            builder: (BuildContext context, Widget? child) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  ListenableBuilder(
                      listenable: widget.chatStateController,
                      builder: (BuildContext context, Widget? child) {
                        return SelectedAttachments(
                          attachments: widget.chatStateController.attachments,
                          onRemove: widget.chatStateController.removeAttachment,
                        );
                      }),
                  const SizedBox(height: 7),
                  SizedBox(
                    height: 50,
                    child: Container(
                      decoration: BoxDecoration(
                        color: Theme.of(context).colorScheme.tertiaryContainer,
                        borderRadius: BorderRadius.circular(10.0),
                        border: Border.all(
                            color: widget.chatStateController.active
                                ? Colors.grey.shade400
                                : Colors.grey.shade600),
                      ),
                      padding: EdgeInsets.symmetric(
                          horizontal:
                              widget.chatStateController.active ? 4 : 12),
                      child: Row(
                        mainAxisAlignment: MainAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (widget.chatStateController.active)
                            IconButton(
                              onPressed: _chatInputController.chooseFile,
                              icon: SvgPicture.asset(
                                'assets/icons/Attachment.svg',
                                semanticsLabel: 'Attachment button icon',
                                width: 26,
                              ),
                              hoverColor: Colors.transparent,
                            ),
                          Flexible(
                              fit: FlexFit.loose,
                              child: TextField(
                                focusNode: _focusNode,
                                controller:
                                    widget.chatStateController.messageInput,
                                enabled: widget.chatStateController.active,
                                onSubmitted: (message) {
                                  sendMessage(message);
                                },
                                decoration: InputDecoration(
                                  labelText: widget.chatStateController.active
                                      ? 'Message'
                                      : 'Chat disabled',
                                  floatingLabelBehavior:
                                      FloatingLabelBehavior.never,
                                  disabledBorder: _noBorder,
                                  border: _noBorder,
                                  focusedBorder: _noBorder,
                                  enabledBorder: _noBorder,
                                  contentPadding:
                                      const EdgeInsets.symmetric(horizontal: 2),
                                ),
                              )),
                          if (widget.chatStateController.active)
                            IconButton(
                              onPressed: () {
                                String message = widget
                                    .chatStateController.messageInput.text;
                                sendMessage(message);
                              },
                              icon: SvgPicture.asset(
                                'assets/icons/Send.svg',
                                semanticsLabel: 'Send button icon',
                                width: 32,
                              ),
                              hoverColor: Colors.transparent,
                            )
                        ],
                      ),
                    ),
                  ),
                ],
              );
            }),
      ],
    );
  }

  void showAttachmentMenu(Offset position, File? file) {
    showDialog(
      context: context,
      barrierColor: Colors.transparent,
      builder: (BuildContext context) {
        return CustomPositionedDialog(position: position, file: file);
      },
    );
  }

  void showImagePreview(Image image) {
    showDialog(
        context: context,
        builder: (BuildContext context) {
          return GestureDetector(
            onTap: () {
              Navigator.of(context).pop();
            },
            child: Stack(
              children: [
                Center(
                  child: InkWell(
                    onTap: () {},
                    child: image,
                  ),
                ),
              ],
            ),
          );
        });
  }
}
