import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/telepathy.dart';

class RoomWidget extends StatefulWidget {
  final Room room;
  final Telepathy telepathy;
  final StateController stateController;
  final SoundPlayer player;

  const RoomWidget({
    super.key,
    required this.room,
    required this.stateController,
    required this.telepathy,
    required this.player,
  });

  @override
  State<StatefulWidget> createState() => RoomWidgetState();
}

class RoomWidgetState extends State<RoomWidget> {
  bool isHovered = false;

  @override
  Widget build(BuildContext context) {
    return InkWell(
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () {},
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
                'assets/icons/Phone.svg',
                semanticsLabel: 'Call icon',
                width: 32,
              ),
              onPressed: () async {
                if (widget.stateController.isCallActive) {
                  showErrorDialog(
                      context, 'Call failed', 'There is a call already active');
                  return;
                } else if (widget.stateController.inAudioTest) {
                  showErrorDialog(context, 'Call failed',
                      'Cannot make a call while in an audio test');
                  return;
                } else if (widget.stateController.callEndedRecently) {
                  // if the call button is pressed right after a call ended, we assume the user did not want to make a call
                  return;
                }

                widget.stateController.setStatus('Connecting');
                List<int> bytes = await readSeaBytes('outgoing');
                outgoingSoundHandle = await widget.player.play(bytes: bytes);

                try {
                  await widget.telepathy
                      .joinRoom(memberStrings: widget.room.peerIds);
                  widget.room.online.clear();
                  widget.stateController.setActiveRoom(widget.room);
                } on DartError catch (e) {
                  widget.stateController.setStatus('Inactive');
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
