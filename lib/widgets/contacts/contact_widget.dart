import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';
import 'package:telepathy/src/rust/flutter.dart';

class _EditContactDialog extends StatelessWidget {
  final Contact contact;
  final TextEditingController nicknameController;
  final StateController stateController;
  final ProfilesController profilesController;
  final Telepathy telepathy;

  const _EditContactDialog({
    required this.contact,
    required this.nicknameController,
    required this.stateController,
    required this.profilesController,
    required this.telepathy,
  });

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final peerId = contact.peerId();

    return SimpleDialog(
      backgroundColor: scheme.secondaryContainer,
      shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(12)),
      contentPadding: const EdgeInsets.fromLTRB(20, 12, 20, 20),
      titlePadding: const EdgeInsets.fromLTRB(20, 20, 20, 8),
      title: Row(
        children: [
          CircleAvatar(
            maxRadius: 22,
            backgroundColor: scheme.tertiaryContainer,
            child: SvgPicture.asset('assets/icons/Profile.svg'),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              contact.nickname(),
              style: const TextStyle(
                  fontSize: 18, fontWeight: FontWeight.w600),
            ),
          ),
        ],
      ),
      children: [
        TextInput(
          enabled: !stateController.isActiveContact(contact),
          controller: nicknameController,
          labelText: 'Nickname',
          onChanged: (value) {
            contact.setNickname(nickname: value);
          },
        ),
        const SizedBox(height: 16),
        Text(
          'Peer ID',
          style: TextStyle(
            fontSize: 12,
            color: scheme.onSecondaryContainer.withValues(alpha: 0.75),
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 6),
        SelectableText(
          peerId,
          style: const TextStyle(fontSize: 13),
        ),
        const SizedBox(height: 8),
        Align(
          alignment: Alignment.centerRight,
          child: Button(
            text: 'Copy peer ID',
            onPressed: () async {
              await Clipboard.setData(ClipboardData(text: peerId));
              if (!context.mounted) return;
              ScaffoldMessenger.of(context).showSnackBar(
                const SnackBar(
                  content: Text('Peer ID copied'),
                  duration: Duration(seconds: 1),
                ),
              );
            },
          ),
        ),
        const SizedBox(height: 20),
        Button(
          text: 'Save',
          onPressed: () {
            profilesController.saveContacts();
            Navigator.pop(context);
          },
        ),
        const SizedBox(height: 20),
        Container(
          width: double.infinity,
          padding: const EdgeInsets.all(14),
          decoration: BoxDecoration(
            color: const Color(0xFFdc2626).withValues(alpha: 0.12),
            borderRadius: BorderRadius.circular(10),
            border: Border.all(
              color: const Color(0xFFdc2626).withValues(alpha: 0.35),
            ),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text(
                'Danger zone',
                style: TextStyle(
                  fontWeight: FontWeight.w600,
                  color: Color(0xFFdc2626),
                ),
              ),
              const SizedBox(height: 10),
              Button(
                text: 'Delete contact',
                onPressed: () async {
                  if (!stateController.isActiveContact(contact)) {
                    bool confirm = await showDialog<bool>(
                            context: context,
                            builder: (BuildContext context) {
                              return SimpleDialog(
                                shape: RoundedRectangleBorder(
                                    borderRadius: BorderRadius.circular(12)),
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
                                    mainAxisAlignment: MainAxisAlignment.end,
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
                      profilesController.removeContact(contact);
                      telepathy.stopSession(contact: contact);
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
              ),
            ],
          ),
        ),
      ],
    );
  }
}

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
    final stateController = context.watch<StateController>();
    final telepathy = context.read<Telepathy>();
    final profilesController = context.read<ProfilesController>();
    final player = context.read<SoundPlayer>();

    bool active = stateController.isActiveContact(widget.contact);
    SessionStatus status = stateController.sessionStatus(widget.contact);
    bool online = status is SessionStatus_Connected;
    bool connecting = status is SessionStatus_Connecting;
    bool inactive = status is SessionStatus_Inactive;
    final connectedStatus = online ? status : null;
    final scheme = Theme.of(context).colorScheme;

    return InkWell(
      mouseCursor: SystemMouseCursors.click,
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () {
        showDialog(
            barrierDismissible: true,
            context: context,
            builder: (BuildContext context) {
              return _EditContactDialog(
                contact: widget.contact,
                nicknameController: _nicknameInput,
                stateController: stateController,
                profilesController: profilesController,
                telepathy: telepathy,
              );
            });
      },
      hoverColor: Colors.transparent,
      child: Container(
        margin: const EdgeInsets.symmetric(horizontal: 6, vertical: 3),
        decoration: BoxDecoration(
          color: scheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6.5),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Stack(
              clipBehavior: Clip.none,
              children: [
                CircleAvatar(
                  maxRadius: 17,
                  child: SvgPicture.asset(isHovered
                      ? 'assets/icons/Edit.svg'
                      : 'assets/icons/Profile.svg'),
                ),
                if (online)
                  Positioned(
                    right: -1,
                    bottom: -1,
                    child: Container(
                      width: 8,
                      height: 8,
                      decoration: BoxDecoration(
                        color: Colors.green,
                        shape: BoxShape.circle,
                        border: Border.all(
                            color: scheme.secondaryContainer, width: 1.5),
                      ),
                    ),
                  ),
              ],
            ),
            const SizedBox(width: 10),
            Text(widget.contact.nickname(),
                style: const TextStyle(fontSize: 16)),
            const Spacer(),
            if (inactive) ...[
              Tooltip(
                message: 'Retry connection',
                child: IconButton(
                  style: IconButton.styleFrom(
                    minimumSize: const Size(48, 48),
                  ),
                  onPressed: () {
                    telepathy.startSession(contact: widget.contact);
                  },
                  icon: SvgPicture.asset('assets/icons/Restart.svg',
                      semanticsLabel: 'Retry the session initiation'),
                ),
              ),
            ],
            if (connecting) ...[
              Tooltip(
                message: 'Connecting…',
                child: SizedBox(
                  width: 18,
                  height: 18,
                  child: CircularProgressIndicator(strokeWidth: 2.5),
                ),
              ),
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
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                decoration: BoxDecoration(
                  color: scheme.primary,
                  borderRadius: BorderRadius.circular(8),
                ),
                child: Text(
                  connectedStatus.relayed ? 'relayed' : 'direct',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w600,
                    color: scheme.onPrimary,
                  ),
                ),
              ),
            ],
            if (active)
              Tooltip(
                message: 'End call',
                child: IconButton(
                  style: IconButton.styleFrom(
                    minimumSize: const Size(48, 48),
                  ),
                  icon: SvgPicture.asset(
                    'assets/icons/PhoneOff.svg',
                    semanticsLabel: 'End call icon',
                    width: 28,
                  ),
                  onPressed: () async {
                    outgoingSoundHandle?.cancel();

                    telepathy.endCall();
                    stateController.endOfCall();

                    List<int> bytes = await readSeaBytes('call_ended');
                    otherSoundHandle = await player.play(bytes: bytes);
                  },
                ),
              ),
            if (!active && online)
              Tooltip(
                message: 'Start voice call',
                child: IconButton(
                  style: IconButton.styleFrom(
                    minimumSize: const Size(48, 48),
                  ),
                  icon: SvgPicture.asset(
                    'assets/icons/Phone.svg',
                    semanticsLabel: 'Call icon',
                    width: 28,
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
                ),
              )
          ],
        ),
      ),
    );
  }
}
