import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/widgets/common/index.dart';

class _EditRoomDialog extends StatefulWidget {
  final Room room;

  const _EditRoomDialog({required this.room});

  @override
  State<_EditRoomDialog> createState() => _EditRoomDialogState();
}

class _EditRoomDialogState extends State<_EditRoomDialog> {
  late TextEditingController _nicknameController;

  @override
  void initState() {
    super.initState();
    _nicknameController =
        TextEditingController(text: widget.room.nickname);
  }

  @override
  void dispose() {
    _nicknameController.dispose();
    super.dispose();
  }

  String _memberLabel(String peerId, ProfilesController pc) {
    final c =
        pc.contacts.values.firstWhereOrNull((x) => x.peerId() == peerId);
    if (c != null) return c.nickname();
    if (peerId == pc.peerId) return 'You';
    if (peerId.length > 16) return '${peerId.substring(0, 16)}…';
    return peerId;
  }

  @override
  Widget build(BuildContext context) {
    final scheme = Theme.of(context).colorScheme;
    final profilesController = context.read<ProfilesController>();
    final stateController = context.read<StateController>();

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
            child: SvgPicture.asset('assets/icons/Group.svg'),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text(
              widget.room.nickname,
              style: const TextStyle(
                  fontSize: 18, fontWeight: FontWeight.w600),
            ),
          ),
        ],
      ),
      children: [
        TextInput(
          controller: _nicknameController,
          labelText: 'Room nickname',
          onChanged: (v) => widget.room.nickname = v,
        ),
        const SizedBox(height: 16),
        Text(
          'Members',
          style: TextStyle(
            fontSize: 12,
            color: scheme.onSecondaryContainer.withValues(alpha: 0.75),
            fontWeight: FontWeight.w600,
          ),
        ),
        const SizedBox(height: 8),
        ...widget.room.peerIds.map((pid) {
          return Padding(
            padding: const EdgeInsets.only(bottom: 6),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Expanded(
                  child: Text(
                    _memberLabel(pid, profilesController),
                    style: const TextStyle(fontSize: 14),
                  ),
                ),
                const SizedBox(width: 8),
                SelectableText(
                  pid,
                  style: TextStyle(
                    fontSize: 11,
                    color:
                        scheme.onSecondaryContainer.withValues(alpha: 0.65),
                  ),
                ),
              ],
            ),
          );
        }),
        const SizedBox(height: 16),
        Button(
          text: 'Save',
          onPressed: () {
            profilesController.saveRooms();
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
                text: 'Delete room',
                onPressed: () async {
                  if (stateController.isActiveRoom(widget.room)) {
                    showErrorDialog(context, 'Warning',
                        'Cannot delete a room while in an active call');
                    return;
                  }

                  final confirm = await showDialog<bool>(
                        context: context,
                        builder: (BuildContext context) {
                          return SimpleDialog(
                            shape: RoundedRectangleBorder(
                                borderRadius: BorderRadius.circular(12)),
                            title: const Text('Delete room?'),
                            contentPadding: const EdgeInsets.only(
                                bottom: 25, left: 25, right: 25),
                            titlePadding: const EdgeInsets.only(
                                top: 25,
                                left: 25,
                                right: 25,
                                bottom: 20),
                            children: [
                              const Text(
                                  'This room will be removed from your list.'),
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
                        },
                      ) ??
                      false;

                  if (confirm && context.mounted) {
                    profilesController.removeRoom(widget.room);
                    Navigator.pop(context);
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

class RoomWidget extends StatefulWidget {
  final Room room;

  const RoomWidget({
    super.key,
    required this.room,
  });

  @override
  State<StatefulWidget> createState() => RoomWidgetState();
}

class RoomWidgetState extends State<RoomWidget> {
  bool isHovered = false;

  @override
  Widget build(BuildContext context) {
    final stateController = context.read<StateController>();
    final telepathy = context.read<Telepathy>();
    final player = context.read<SoundPlayer>();
    final scheme = Theme.of(context).colorScheme;
    final count = widget.room.peerIds.length;

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
          builder: (context) => _EditRoomDialog(room: widget.room),
        );
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
            CircleAvatar(
              maxRadius: 17,
              child: SvgPicture.asset(isHovered
                  ? 'assets/icons/Edit.svg'
                  : 'assets/icons/Group.svg'),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Text(
                widget.room.nickname,
                style: const TextStyle(fontSize: 16),
                overflow: TextOverflow.ellipsis,
              ),
            ),
            Container(
              margin: const EdgeInsets.only(right: 6),
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
              decoration: BoxDecoration(
                color: scheme.primary,
                borderRadius: BorderRadius.circular(12),
              ),
              child: Text(
                '$count',
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w700,
                  color: scheme.onPrimary,
                ),
              ),
            ),
            Tooltip(
              message: 'Copy room invite',
              child: IconButton(
                style: IconButton.styleFrom(
                  minimumSize: const Size(48, 48),
                ),
                icon: SvgPicture.asset(
                  'assets/icons/Copy.svg',
                  semanticsLabel: 'Copy room details icon',
                  width: 26,
                ),
                onPressed: () async {
                  try {
                    final roomDetailsString = widget.room.toShareableFormat();
                    await Clipboard.setData(
                        ClipboardData(text: roomDetailsString));
                    if (!context.mounted) return;
                    ScaffoldMessenger.of(context).showSnackBar(
                      const SnackBar(
                        content: Text('Room details copied'),
                        duration: Duration(seconds: 1),
                      ),
                    );
                  } catch (_) {
                    if (!context.mounted) return;
                    showErrorDialog(context, 'Copy failed',
                        'Failed to copy room details to clipboard');
                  }
                },
              ),
            ),
            Tooltip(
              message: 'Join room call',
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
                    showErrorDialog(
                        context, 'Call failed', 'There is a call already active');
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
                    await telepathy.joinRoom(memberStrings: widget.room.peerIds);
                    widget.room.online.clear();
                    stateController.setActiveRoom(widget.room);
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
