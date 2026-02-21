import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:telepathy/widgets/chat/message_item.dart';
import 'package:telepathy/widgets/chat/selected_attachments.dart';

class ChatWidget extends StatefulWidget {
  const ChatWidget({super.key});

  @override
  State<StatefulWidget> createState() => ChatWidgetState();
}

class ChatWidgetState extends State<ChatWidget> {
  static const OutlineInputBorder _noBorder = OutlineInputBorder(
    borderSide: BorderSide(color: Colors.transparent),
  );

  final FocusNode _focusNode = FocusNode();
  late final ChatInputController _chatInputController;
  late final StateController _stateController;

  @override
  void initState() {
    super.initState();
    _stateController = context.read<StateController>();
    _stateController.addListener(_onStateControllerChange);

    _chatInputController = ChatInputController(
      chatStateController: context.read<ChatStateController>(),
      focusNode: _focusNode,
    )..init();

    // NOTE: keyboard focus + paste handling are managed by [_chatInputController].
  }

  @override
  void dispose() {
    _stateController.removeListener(_onStateControllerChange);
    _chatInputController.dispose();
    _focusNode.dispose();
    super.dispose();
  }

  void sendMessage(String text) async {
    final chatStateController = context.read<ChatStateController>();
    final telepathy = context.read<Telepathy>();

    if (!chatStateController.active) return;
    if (text.isEmpty && chatStateController.attachments.isEmpty) return;

    final Contact? contact = _stateController.activeContact;
    // Chat is only supported for direct contact calls right now (not rooms).
    if (contact == null) return;

    try {
      ChatMessage message = telepathy.buildChat(
          contact: contact,
          text: text,
          attachments: chatStateController.attachments);
      await telepathy.sendChat(message: message);

      message.clearAttachments();
      chatStateController.messages.add(message);
      chatStateController.clearInput();
    } on DartError catch (error) {
      if (!mounted) return;
      showErrorDialog(context, 'Message Send Failed', error.message);
    }
  }

  void _onStateControllerChange() {
    final chatStateController = context.read<ChatStateController>();

    // Only enable chat when a direct contact is active.
    final bool desiredActive = _stateController.activeContact != null;

    if (desiredActive == chatStateController.active) {
      return;
    } else if (!desiredActive && chatStateController.active) {
      chatStateController.clearState();
    }

    setState(() {
      chatStateController.active = desiredActive;
    });
  }

  @override
  Widget build(BuildContext context) {
    final chatStateController = context.watch<ChatStateController>();
    final profilesController = context.read<ProfilesController>();

    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Flexible(
            fit: FlexFit.loose,
            child: ListView.builder(
                itemCount: chatStateController.messages.length,
                itemBuilder: (BuildContext context, int index) {
                  ChatMessage message = chatStateController.messages[index];
                  bool sender = message.isSender(
                      identity: profilesController.peerId);

                  return MessageItem(
                    message: message,
                    isSender: sender,
                    files: chatStateController.files,
                    onShowAttachmentMenu: showAttachmentMenu,
                    onShowImagePreview: showImagePreview,
                  );
                })),
        Consumer<StateController>(
            builder:
                (BuildContext context, StateController stateController, _) {
              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  SelectedAttachments(
                    attachments: chatStateController.attachments,
                    onRemove: chatStateController.removeAttachment,
                  ),
                  const SizedBox(height: 7),
                  SizedBox(
                    height: 50,
                    child: Container(
                      decoration: BoxDecoration(
                        color: Theme.of(context).colorScheme.tertiaryContainer,
                        borderRadius: BorderRadius.circular(10.0),
                        border: Border.all(
                            color: chatStateController.active
                                ? Colors.grey.shade400
                                : Colors.grey.shade600),
                      ),
                      padding: EdgeInsets.symmetric(
                          horizontal:
                              chatStateController.active ? 4 : 12),
                      child: Row(
                        mainAxisAlignment: MainAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (chatStateController.active)
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
                                    chatStateController.messageInput,
                                enabled: chatStateController.active,
                                onSubmitted: (message) {
                                  sendMessage(message);
                                },
                                decoration: InputDecoration(
                                  labelText: chatStateController.active
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
                          if (chatStateController.active)
                            IconButton(
                              onPressed: () {
                                String message =
                                    chatStateController.messageInput.text;
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
