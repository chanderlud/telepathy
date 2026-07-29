import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/core/rust/types.dart';

import 'call_start_lifecycle.dart';

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
  late TextEditingController _directConnInput;

  @override
  void initState() {
    super.initState();
    _nicknameInput = TextEditingController(text: widget.contact.nickname());
    _directConnInput = TextEditingController();
  }

  @override
  void didUpdateWidget(ContactWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.contact != oldWidget.contact) {
      _nicknameInput.text = widget.contact.nickname();
      _directConnInput.clear();
    }
  }

  @override
  void dispose() {
    _nicknameInput.dispose();
    _directConnInput.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final stateController = context.watch<StateController>();
    final telepathy = context.read<Telepathy>();
    final profilesController = context.read<ProfilesController>();
    final player = context.read<SoundPlayer>();

    bool active = stateController.isActiveContact(widget.contact);
    bool pending = stateController.pendingContact?.id() == widget.contact.id();
    SessionStatus status = stateController.sessionStatus(widget.contact);
    bool online = status is SessionStatus_Connected;
    bool connecting = status is SessionStatus_Connecting;
    bool inactive = status is SessionStatus_Inactive;
    final connectedStatus = online ? status : null;

    return InkWell(
      mouseCursor: SystemMouseCursors.click,
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () {
        double contactOutputVolume = widget.contact.outputVolume();

        showDialog(
            barrierDismissible: false,
            context: context,
            builder: (BuildContext context) {
              return StatefulBuilder(
                  builder: (BuildContext context, StateSetter setDialogState) {
                return SimpleDialog(
                  title: Row(
                    mainAxisAlignment: MainAxisAlignment.spaceBetween,
                    children: [
                      const Text('Edit Contact'),
                      IconButton(
                        onPressed: () async {
                          // Lifecycle-lock: deleting a pending target would
                          // remove the only frontend hangup control while the
                          // backend start request is still in flight. The
                          // target must remain editable-only after the
                          // lifecycle returns to idle, matching the active
                          // target gate.
                          final bool isCallTarget =
                              stateController.isActiveContact(widget.contact) ||
                                  stateController.pendingContact?.id() ==
                                      widget.contact.id();
                          if (!isCallTarget) {
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
                            showErrorDialog(
                                context,
                                'Warning',
                                stateController.isActiveContact(widget.contact)
                                    ? 'Cannot delete a contact while in an active call'
                                    : 'Cannot delete a contact while a call is being placed');
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
                        enabled:
                            !(stateController.isActiveContact(widget.contact) ||
                                stateController.pendingContact?.id() ==
                                    widget.contact.id()),
                        controller: _nicknameInput,
                        labelText: 'Nickname',
                        onChanged: (value) {
                          widget.contact.setNickname(nickname: value);
                        }),
                    const SizedBox(height: 12),
                    StatefulBuilder(builder: (context, setLocalState) {
                      final bool isDirect = widget.contact.isDirect();
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            isDirect
                                ? 'Direct connection is enabled'
                                : 'Use a direct invitation to enable direct connection.',
                          ),
                          const SizedBox(height: 12),
                          if (!isDirect) ...[
                            TextInput(
                              controller: _directConnInput,
                              labelText: 'Direct invitation',
                              hintText: 'Paste a tp1: invitation',
                            ),
                            const SizedBox(height: 8),
                            Button(
                              text: 'Use invitation',
                              onPressed: () {
                                final invitation = _directConnInput.text.trim();
                                if (invitation.isEmpty) {
                                  showErrorDialog(
                                    context,
                                    'Direct invitation required',
                                    'Paste a tp1: invitation to enable direct connection.',
                                  );
                                  return;
                                }
                                try {
                                  widget.contact.setDirectInvitation(
                                    invitation: invitation,
                                  );
                                  widget.contact.setDirect(isDirect: true);
                                  _directConnInput.clear();
                                  setLocalState(() {});
                                  setDialogState(() {});
                                } on DartError catch (_) {
                                  showErrorDialog(
                                    context,
                                    'Invalid direct invitation',
                                    'This invitation is invalid or belongs to a different contact. Paste a valid tp1: invitation.',
                                  );
                                }
                              },
                            ),
                          ] else
                            Button(
                              text: 'Remove direct invitation',
                              onPressed: () {
                                widget.contact.setDirectInvitation();
                                widget.contact.setDirect(isDirect: false);
                                setLocalState(() {});
                                setDialogState(() {});
                              },
                            ),
                        ],
                      );
                    }),
                    const SizedBox(height: 20),
                    const Text('Output Volume', style: TextStyle(fontSize: 15)),
                    Slider(
                        value: contactOutputVolume,
                        onChanged: (value) {
                          setDialogState(() {
                            contactOutputVolume = value;
                          });
                          widget.contact.setOutputVolume(decibel: value);
                          telepathy.setContactOutputVolume(
                            contact: widget.contact,
                          );
                        },
                        onChangeEnd: (_) {
                          profilesController.saveContacts();
                        },
                        min: -15,
                        max: 15,
                        label: '${contactOutputVolume.toStringAsFixed(2)} db'),
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
            if (online && connectedStatus != null) ...[
              Text(connectedStatus.relayed ? 'relayed' : 'direct'),
              const SizedBox(width: 5),
              Text(connectedStatus.remoteAddress),
            ],
            if (active || pending)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/PhoneOff.svg',
                  semanticsLabel: 'End call icon',
                  width: 32,
                ),
                onPressed: () async {
                  outgoingSoundHandle?.cancel();

                  stateController.cancelCurrentStartOperation();
                  if (!stateController.beginCallEnding()) return;
                  await telepathy.endCall();
                  stateController.endOfCall();

                  List<int> bytes = await readSeaBytes('call_ended');
                  otherSoundHandle = await playSoundEffect(
                    player: player,
                    bytes: bytes,
                    sound: 'call-ended',
                  );
                },
              ),
            if (!active && !pending && online)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/Phone.svg',
                  semanticsLabel: 'Call icon',
                  width: 32,
                ),
                onPressed: () async {
                  if (stateController.hasLiveCall) {
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

                  // Capture the target before any await so the continuation
                  // and the `callState` gate observe the same contact the
                  // user clicked, even if the widget is rebuilt against a
                  // different contact while `startCall` is still resolving.
                  final Contact target = widget.contact;
                  final operation = telepathy.newStartOperation();
                  stateController.setStatus('Connecting');
                  final attempt =
                      stateController.setPendingContact(target, operation);

                  await runOutgoingCallStartLifecycle(
                    context: context,
                    stateController: stateController,
                    player: player,
                    attempt: attempt,
                    startRequest: () => telepathy.startCall(
                      contact: target,
                      operation: operation,
                    ),
                  );
                },
              )
          ],
        ),
      ),
    );
  }
}
