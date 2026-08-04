import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/core/rust/player.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/rust/types.dart';
import 'package:telepathy/models/index.dart';

const Color onlineDotColor = Color(0xFF22c55e);
const Color offlineDotColor = Color(0xFF6b7280);

/// The panel shown while a room call is active: the room's name, a hangup
/// control, and its members split into online and offline groups.
class RoomDetailsWidget extends StatelessWidget {
  const RoomDetailsWidget({super.key});

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final stateController = context.watch<StateController>();
    final player = context.read<SoundPlayer>();
    final profilesController = context.watch<ProfilesController>();

    final Room? room = stateController.activeRoom;
    if (room == null) return const SizedBox.shrink();

    String nicknameOf(String peerId) {
      final Contact? contact = profilesController.contacts.values
          .firstWhereOrNull((c) => c.peerId() == peerId);
      if (contact != null) {
        return contact.nickname();
      } else if (peerId == profilesController.peerId) {
        return 'You';
      } else {
        return 'Anonymous';
      }
    }

    final List<String> online = [...room.online, profilesController.peerId];
    final List<String> offline =
        room.peerIds.where((p) => !online.contains(p)).toList();

    return Container(
      padding: const EdgeInsets.only(bottom: 12, left: 12, right: 12, top: 8),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    room.nickname,
                    style: const TextStyle(fontSize: 20),
                    overflow: TextOverflow.ellipsis,
                    maxLines: 1,
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  '${online.length}/${room.peerIds.length} online',
                  style: TextStyle(fontSize: 13, color: Colors.grey.shade400),
                ),
                const SizedBox(width: 4),
                IconButton(
                  visualDensity: VisualDensity.comfortable,
                  tooltip: 'Leave room',
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
              ],
            ),
          ),
          Flexible(
            child: SingleChildScrollView(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                mainAxisSize: MainAxisSize.min,
                children: [
                  _MemberSection(
                    label: 'Online',
                    color: onlineDotColor,
                    names: online.map(nicknameOf).toList(),
                  ),
                  if (offline.isNotEmpty)
                    _MemberSection(
                      label: 'Offline',
                      color: offlineDotColor,
                      names: offline.map(nicknameOf).toList(),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

class _MemberSection extends StatelessWidget {
  final String label;
  final Color color;
  final List<String> names;

  const _MemberSection({
    required this.label,
    required this.color,
    required this.names,
  });

  @override
  Widget build(BuildContext context) {
    if (names.isEmpty) return const SizedBox.shrink();

    return Padding(
      padding: const EdgeInsets.only(left: 8, right: 8, top: 4, bottom: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            // Count-first phrasing matches the header's "N/M online" counter.
            '${names.length} ${label.toLowerCase()}',
            style: TextStyle(fontSize: 12, color: Colors.grey.shade400),
          ),
          const SizedBox(height: 6),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              for (final String name in names)
                MemberStatusChip(name: name, dotColor: color),
            ],
          ),
        ],
      ),
    );
  }
}

/// A pill showing one room member: a status dot plus their nickname.
class MemberStatusChip extends StatelessWidget {
  final String name;
  final Color dotColor;

  const MemberStatusChip(
      {super.key, required this.name, required this.dotColor});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.tertiaryContainer,
        borderRadius: BorderRadius.circular(14),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 8,
            height: 8,
            decoration: BoxDecoration(color: dotColor, shape: BoxShape.circle),
          ),
          const SizedBox(width: 7),
          Flexible(
            child: Text(
              name,
              style: const TextStyle(fontSize: 13),
              overflow: TextOverflow.ellipsis,
              maxLines: 1,
            ),
          ),
        ],
      ),
    );
  }
}
