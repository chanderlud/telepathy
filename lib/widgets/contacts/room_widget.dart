import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/widgets/call/room_details_widget.dart';
import 'package:telepathy/widgets/common/index.dart';

import 'call_start_lifecycle.dart';

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
  late TextEditingController _nicknameInput;

  @override
  void initState() {
    super.initState();
    _nicknameInput = TextEditingController(text: widget.room.nickname);
  }

  @override
  void didUpdateWidget(RoomWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.room != oldWidget.room) {
      _nicknameInput.text = widget.room.nickname;
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
    final active = stateController.isActiveRoom(widget.room);
    final pending = stateController.pendingRoom?.id == widget.room.id;

    return InkWell(
      mouseCursor: SystemMouseCursors.click,
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () => _showEditDialog(
        context,
        stateController: stateController,
        profilesController: profilesController,
      ),
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
                  : 'assets/icons/Group.svg'),
            ),
            const SizedBox(width: 10),
            // Slack absorber, same pattern as ContactWidget: tight slot
            // keeps the buttons flush right, text ellipsizes when narrow.
            Expanded(
              child: Text(widget.room.nickname,
                  style: const TextStyle(fontSize: 16),
                  overflow: TextOverflow.ellipsis,
                  maxLines: 1),
            ),
            IconButton(
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Copy.svg',
                semanticsLabel: 'Copy room details icon',
                width: 28,
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
            if (!active && !pending)
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
                  // and the `callState` gate observe the same room the user
                  // clicked, even if the widget is rebuilt against a
                  // different room while `joinRoom` is still resolving.
                  final Room target = widget.room;
                  final operation = telepathy.newStartOperation();
                  stateController.setStatus('Connecting');
                  final attempt =
                      stateController.setPendingRoom(target, operation);

                  await runOutgoingCallStartLifecycle(
                    context: context,
                    stateController: stateController,
                    player: player,
                    attempt: attempt,
                    startRequest: () => telepathy.joinRoom(
                      memberStrings: target.peerIds,
                      operation: operation,
                    ),
                    onStartAccepted: target.online.clear,
                  );
                },
              )
          ],
        ),
      ),
    );
  }

  Future<void> _showEditDialog(
    BuildContext context, {
    required StateController stateController,
    required ProfilesController profilesController,
  }) {
    return showDialog(
      barrierDismissible: false,
      context: context,
      builder: (BuildContext dialogContext) {
        return SimpleDialog(
          title: Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              const Text('Edit Room'),
              IconButton(
                onPressed: () async {
                  // Lifecycle-lock: deleting a pending room would remove the
                  // only frontend hangup control while the backend join is
                  // still in flight. The target must remain until the
                  // lifecycle returns to idle, matching the active target
                  // gate.
                  final bool isCallTarget =
                      stateController.isActiveRoom(widget.room) ||
                          stateController.pendingRoom?.id == widget.room.id;
                  if (!isCallTarget) {
                    bool confirm = await _confirmDelete(dialogContext);
                    if (confirm) {
                      profilesController.removeRoom(widget.room);
                    }
                    if (dialogContext.mounted) {
                      Navigator.pop(dialogContext);
                    }
                  } else {
                    showErrorDialog(
                      dialogContext,
                      'Warning',
                      stateController.isActiveRoom(widget.room)
                          ? 'Cannot delete a room while in an active call'
                          : 'Cannot delete a room while a call is being placed',
                    );
                  }
                },
                icon: SvgPicture.asset('assets/icons/Trash.svg',
                    semanticsLabel: 'Delete room icon'),
              ),
            ],
          ),
          contentPadding:
              const EdgeInsets.only(bottom: 25, left: 25, right: 25),
          titlePadding:
              const EdgeInsets.only(top: 25, left: 25, right: 25, bottom: 20),
          children: [
            TextInput(
                enabled: !(stateController.isActiveRoom(widget.room) ||
                    stateController.pendingRoom?.id == widget.room.id),
                controller: _nicknameInput,
                labelText: 'Nickname'),
            const SizedBox(height: 16),
            Text('${widget.room.peerIds.length} members',
                style: TextStyle(fontSize: 13, color: Colors.grey.shade400)),
            const SizedBox(height: 8),
            Wrap(
              spacing: 8,
              runSpacing: 8,
              children: [
                for (final String peerId in widget.room.peerIds)
                  MemberStatusChip(
                    name: _memberName(profilesController, peerId),
                    dotColor: peerId == profilesController.peerId ||
                            _isOnline(stateController, peerId)
                        ? onlineDotColor
                        : offlineDotColor,
                  ),
              ],
            ),
            const SizedBox(height: 20),
            Button(
              text: 'Save',
              onPressed: () {
                final bool isCallTarget =
                    stateController.isActiveRoom(widget.room) ||
                        stateController.pendingRoom?.id == widget.room.id;
                if (isCallTarget) {
                  showErrorDialog(
                    dialogContext,
                    'Warning',
                    stateController.isActiveRoom(widget.room)
                        ? 'Cannot rename a room while in an active call'
                        : 'Cannot rename a room while a call is being placed',
                  );
                  return;
                }

                setState(() {
                  widget.room.nickname = _nicknameInput.text;
                });
                profilesController.saveRooms();
                Navigator.pop(dialogContext);
              },
            ),
          ],
        );
      },
    );
  }

  String _memberName(ProfilesController profilesController, String peerId) {
    if (peerId == profilesController.peerId) return 'You';
    return profilesController.contacts[peerId]?.nickname() ?? 'Anonymous';
  }

  bool _isOnline(StateController stateController, String peerId) {
    if (stateController.isActiveRoom(widget.room)) {
      return widget.room.online.contains(peerId);
    }
    return stateController.sessions[peerId] is SessionStatus_Connected;
  }

  Future<bool> _confirmDelete(BuildContext context) async {
    return await showDialog<bool>(
          context: context,
          builder: (BuildContext dialogContext) {
            return SimpleDialog(
              title: const Text('Warning'),
              contentPadding:
                  const EdgeInsets.only(bottom: 25, left: 25, right: 25),
              titlePadding: const EdgeInsets.only(
                  top: 25, left: 25, right: 25, bottom: 20),
              children: [
                const Text('Are you sure you want to delete this room?'),
                const SizedBox(height: 20),
                Row(
                  mainAxisAlignment: MainAxisAlignment.end,
                  children: [
                    Button(
                      text: 'Cancel',
                      onPressed: () => Navigator.pop(dialogContext, false),
                    ),
                    const SizedBox(width: 10),
                    Button(
                      text: 'Delete',
                      onPressed: () => Navigator.pop(dialogContext, true),
                    ),
                  ],
                ),
              ],
            );
          },
        ) ??
        false;
  }
}
