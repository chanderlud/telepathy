import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/src/rust/flutter.dart';

class RoomDetailsWidget extends StatelessWidget {
  final Telepathy telepathy;
  final StateController stateController;
  final SoundPlayer player;
  final SettingsController settingsController;

  const RoomDetailsWidget(
      {super.key,
      required this.telepathy,
      required this.stateController,
      required this.player,
      required this.settingsController});

  @override
  Widget build(BuildContext context) {
    String getNickname(String peerId) {
      Contact? contact = settingsController.contacts.values
          .firstWhereOrNull((c) => c.peerId() == peerId);
      if (contact != null) {
        return contact.nickname();
      } else if (peerId == settingsController.peerId) {
        return 'You';
      } else {
        return 'Anonymous';
      }
    }

    final room = stateController.activeRoom;
    List<String> online = [...room?.online ?? [], settingsController.peerId];
    var offline = room?.peerIds.where((p) => !online.contains(p)) ?? [];

    return Container(
      padding: const EdgeInsets.only(bottom: 15, left: 12, right: 12, top: 8),
      height: 300,
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          const Padding(
            padding: EdgeInsets.symmetric(horizontal: 8, vertical: 7),
            child: Row(
              children: [
                Text('Room Details', style: TextStyle(fontSize: 20)),
              ],
            ),
          ),
          const SizedBox(height: 10.0),
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
          Text('Online: ${online.map(getNickname).join(' ')}'),
          Text('Offline: ${offline.map(getNickname).join('  ')}')
        ],
      ),
    );
  }
}
