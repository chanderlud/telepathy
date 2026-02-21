import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';

/// A widget which displays a single contact.
class ContactWidget extends StatefulWidget {
  final Contact contact;

  const ContactWidget({super.key, required this.contact});

  @override
  State<StatefulWidget> createState() => ContactWidgetState();
}

class ContactWidgetState extends State<ContactWidget> {
  bool isHovered = false;
  late TextEditingController _nicknameInput;

  @override
  void initState() {
    super.initState();
    _nicknameInput = TextEditingController(text: widget.contact.nickname());
  }

  @override
  void didUpdateWidget(ContactWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.contact != oldWidget.contact) {
      _nicknameInput.text = widget.contact.nickname();
    }
  }

  @override
  void dispose() {
    _nicknameInput.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final stateController = context.read<StateController>();
    final telepathy = context.read<Telepathy>();
    final profilesController = context.read<ProfilesController>();
    final player = context.read<SoundPlayer>();

    bool active = stateController.isActiveContact(widget.contact);
    SessionStatus status = stateController.sessionStatus(widget.contact);
    bool online = status.runtimeType == SessionStatus_Connected;
    bool connecting = status.runtimeType == SessionStatus_Connecting;
    bool inactive = status.runtimeType == SessionStatus_Inactive;

    return InkWell(
      mouseCursor: SystemMouseCursors.click,
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () {
        showDialog(
            barrierDismissible: false,
            context: context,
            builder: (BuildContext context) {
              return SimpleDialog(
                title: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    const Text('Edit Contact'),
                    IconButton(
                      onPressed: () async {
                        if (!stateController.isActiveContact(widget.contact)) {
                          bool confirm = await showDialog<bool>(
                                  context: context,
                                  builder: (BuildContext context) {
                                    return SimpleDialog(
                                      title: const Text('Warning'),
                                      contentPadding: const EdgeInsets.only(
                                          bottom: 25, left: 25, right: 25),
                                      titlePadding: const EdgeInsets.only(
                                          top: 25,
                                          left: 25,
                                          right: 25,
                                          bottom: 20),
                                      children: [
                                        const Text(
                                            'Are you sure you want to delete this contact?'),
                                        const SizedBox(height: 20),
                                        Row(
                                          mainAxisAlignment:
                                              MainAxisAlignment.end,
                                          children: [
                                            Button(
                                              text: 'Cancel',
                                              onPressed: () {
                                                Navigator.pop(context, false);
                                              },
                                            ),
                                            const SizedBox(width: 10),
                                            Button(
                                              text: 'Delete',
                                              onPressed: () {
                                                Navigator.pop(context, true);
                                              },
                                            ),
                                          ],
                                        ),
                                      ],
                                    );
                                  }) ??
                              false;

                          if (confirm) {
                            profilesController.removeContact(widget.contact);
                            telepathy.stopSession(contact: widget.contact);
                            profilesController.saveContacts();
                          }

                          if (context.mounted) {
                            Navigator.pop(context);
                          }
                        } else {
                          showErrorDialog(context, 'Warning',
                              'Cannot delete a contact while in an active call');
                        }
                      },
                      icon: SvgPicture.asset('assets/icons/Trash.svg',
                          semanticsLabel: 'Delete contact icon'),
                    ),
                  ],
                ),
                contentPadding:
                    const EdgeInsets.only(bottom: 25, left: 25, right: 25),
                titlePadding: const EdgeInsets.only(
                    top: 25, left: 25, right: 25, bottom: 20),
                children: [
                  TextInput(
                      enabled: !stateController.isActiveContact(widget.contact),
                      controller: _nicknameInput,
                      labelText: 'Nickname',
                      onChanged: (value) {
                        widget.contact.setNickname(nickname: value);
                      }),
                  const SizedBox(height: 20),
                  Button(
                    text: 'Save',
                    onPressed: () {
                      profilesController.saveContacts();
                      Navigator.pop(context);
                    },
                  ),
                ],
              );
            });
      },
      hoverColor: Colors.transparent,
      child: Container(
        margin: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6.5),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircleAvatar(
              maxRadius: 17,
              child: SvgPicture.asset(isHovered
                  ? 'assets/icons/Edit.svg'
                  : 'assets/icons/Profile.svg'),
            ),
            const SizedBox(width: 10),
            Text(widget.contact.nickname(),
                style: const TextStyle(fontSize: 16)),
            const Spacer(),
            if (inactive) ...[
              IconButton(
                  onPressed: () {
                    telepathy.startSession(contact: widget.contact);
                  },
                  icon: SvgPicture.asset('assets/icons/Restart.svg',
                      semanticsLabel: 'Retry the session initiation')),
              const SizedBox(width: 4)
            ],
            if (connecting) ...[
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 10),
                child: SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(strokeWidth: 3)),
              ),
              const SizedBox(width: 10)
            ],
            if (!online && !connecting)
              Padding(
                  padding: const EdgeInsets.only(left: 7, right: 10),
                  child: SvgPicture.asset(
                    'assets/icons/Offline.svg',
                    semanticsLabel: 'Offline icon',
                    width: 26,
                  )),
            if (online) ...[
              Text((status as SessionStatus_Connected).relayed
                  ? 'relayed'
                  : 'direct'),
              const SizedBox(width: 5),
              Text(status.remoteAddress),
            ],
            if (active)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/PhoneOff.svg',
                  semanticsLabel: 'End call icon',
                  width: 32,
                ),
                onPressed: () async {
                  outgoingSoundHandle?.cancel();

                  telepathy.endCall();
                  stateController.endOfCall();

                  List<int> bytes = await readSeaBytes('call_ended');
                  otherSoundHandle = await player.play(bytes: bytes);
                },
              ),
            if (!active && online)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/Phone.svg',
                  semanticsLabel: 'Call icon',
                  width: 32,
                ),
                onPressed: () async {
                  if (stateController.isCallActive) {
                    showErrorDialog(context, 'Call failed',
                        'There is a call already active');
                    return;
                  } else if (stateController.inAudioTest) {
                    showErrorDialog(context, 'Call failed',
                        'Cannot make a call while in an audio test');
                    return;
                  } else if (stateController.callEndedRecently) {
                    // if the call button is pressed right after a call ended, we assume the user did not want to make a call
                    return;
                  }

                  stateController.setStatus('Connecting');
                  List<int> bytes = await readSeaBytes('outgoing');
                  outgoingSoundHandle = await player.play(bytes: bytes);

                  try {
                    await telepathy.startCall(contact: widget.contact);
                    stateController.setActiveContact(widget.contact);
                  } on DartError catch (e) {
                    stateController.setStatus('Inactive');
                    outgoingSoundHandle?.cancel();
                    if (!context.mounted) return;
                    showErrorDialog(context, 'Call failed', e.message);
                  }
                },
              )
          ],
        ),
      ),
    );
  }
}
