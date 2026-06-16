import 'package:flutter/material.dart';
import 'package:flutter/services.dart' hide TextInput;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/widgets/common/index.dart';

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
                    const Text('Edit Room'),
                    IconButton(
                      onPressed: () async {
                        if (!stateController.isActiveRoom(widget.room)) {
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
                                            'Are you sure you want to delete this room?'),
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
                            profilesController.removeRoom(widget.room);
                          }

                          if (context.mounted) {
                            Navigator.pop(context);
                          }
                        } else {
                          showErrorDialog(context, 'Warning',
                              'Cannot delete a room while in an active call');
                        }
                      },
                      icon: SvgPicture.asset('assets/icons/Trash.svg',
                          semanticsLabel: 'Delete room icon'),
                    ),
                  ],
                ),
                contentPadding:
                    const EdgeInsets.only(bottom: 25, left: 25, right: 25),
                titlePadding: const EdgeInsets.only(
                    top: 25, left: 25, right: 25, bottom: 20),
                children: [
                  TextInput(
                      enabled: !stateController.isActiveRoom(widget.room),
                      controller: _nicknameInput,
                      labelText: 'Nickname'),
                  const SizedBox(height: 20),
                  Button(
                    text: 'Save',
                    onPressed: () {
                      if (stateController.isActiveRoom(widget.room)) {
                        showErrorDialog(context, 'Warning',
                            'Cannot rename a room while in an active call');
                        return;
                      }

                      setState(() {
                        widget.room.nickname = _nicknameInput.text;
                      });
                      profilesController.saveRooms();
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
                  : 'assets/icons/Group.svg'),
            ),
            const SizedBox(width: 10),
            Text(widget.room.nickname, style: const TextStyle(fontSize: 16)),
            const Spacer(),
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
            IconButton(
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Phone.svg',
                semanticsLabel: 'Call icon',
                width: 32,
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
                  // if the call button is pressed right after a call ended, we assume the user did not want to make a call
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
            )
          ],
        ),
      ),
    );
  }
}
