import 'package:collection/collection.dart';
import 'package:flutter/material.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/utils/index.dart';
import 'package:telepathy/src/rust/audio/player.dart';
import 'package:telepathy/src/rust/telepathy.dart';
import 'package:telepathy/src/rust/flutter.dart';

class RoomDetailsWidget extends StatelessWidget {
  const RoomDetailsWidget({super.key});

  String _nicknameFor(
      String peerId, ProfilesController profilesController) {
    Contact? contact = profilesController.contacts.values
        .firstWhereOrNull((c) => c.peerId() == peerId);
    if (contact != null) {
      return contact.nickname();
    } else if (peerId == profilesController.peerId) {
      return 'You';
    } else {
      return 'Anonymous';
    }
  }

  Widget _memberTile({
    required BuildContext context,
    required String peerId,
    required String nickname,
    required bool online,
    required bool isSelf,
    required ColorScheme scheme,
  }) {
    final statusColor = online ? Colors.green : Colors.grey;
    final statusLabel = online ? 'Online' : 'Offline';

    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6),
      child: Row(
        children: [
          Stack(
            clipBehavior: Clip.none,
            children: [
              CircleAvatar(
                maxRadius: 20,
                backgroundColor: scheme.tertiaryContainer,
                child: SvgPicture.asset(
                  'assets/icons/Profile.svg',
                  width: 22,
                  height: 22,
                ),
              ),
              Positioned(
                right: 0,
                bottom: 0,
                child: Container(
                  width: 10,
                  height: 10,
                  decoration: BoxDecoration(
                    color: statusColor,
                    shape: BoxShape.circle,
                    border: Border.all(color: scheme.secondaryContainer, width: 2),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(width: 12),
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(
                    text: nickname,
                    style: const TextStyle(
                        fontSize: 15, fontWeight: FontWeight.w500),
                  ),
                  if (isSelf)
                    TextSpan(
                      text: ' (You)',
                      style: TextStyle(
                        fontSize: 14,
                        color: scheme.onSecondaryContainer
                            .withValues(alpha: 0.7),
                      ),
                    ),
                ],
              ),
            ),
          ),
          Text(
            statusLabel,
            style: TextStyle(
              fontSize: 12,
              color: statusColor.withValues(alpha: 0.9),
              fontWeight: FontWeight.w500,
            ),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final telepathy = context.read<Telepathy>();
    final stateController = context.watch<StateController>();
    final player = context.read<SoundPlayer>();
    final profilesController = context.watch<ProfilesController>();
    final scheme = Theme.of(context).colorScheme;

    final room = stateController.activeRoom;
    if (room == null) {
      return const SizedBox.shrink();
    }

    List<String> online = [...room.online, profilesController.peerId];
    online = online.toSet().toList();
    final offline =
        room.peerIds.where((p) => !online.contains(p)).toList();

    return Container(
      padding: const EdgeInsets.only(bottom: 12, left: 12, right: 12, top: 8),
      constraints: const BoxConstraints(minHeight: 120),
      decoration: BoxDecoration(
        color: scheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      room.nickname,
                      style: const TextStyle(
                        fontSize: 22,
                        fontWeight: FontWeight.bold,
                      ),
                    ),
                  ],
                ),
              ),
              IconButton(
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
            ],
          ),
          const SizedBox(height: 12),
          Expanded(
            child: Container(
              decoration: BoxDecoration(
                color: scheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(8),
              ),
              child: ClipRRect(
                borderRadius: BorderRadius.circular(8),
                child: ListView(
                  padding: const EdgeInsets.symmetric(vertical: 8),
                  children: [
                    if (online.isNotEmpty) ...[
                      Padding(
                        padding:
                            const EdgeInsets.fromLTRB(14, 4, 14, 8),
                        child: Text(
                          'Online (${online.length})',
                          style: const TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: Colors.green,
                            letterSpacing: 0.4,
                          ),
                        ),
                      ),
                      ...online.map((peerId) {
                        final isSelf = peerId == profilesController.peerId;
                        return _memberTile(
                          context: context,
                          peerId: peerId,
                          nickname: _nicknameFor(peerId, profilesController),
                          online: true,
                          isSelf: isSelf,
                          scheme: scheme,
                        );
                      }),
                    ],
                    if (online.isNotEmpty && offline.isNotEmpty) ...[
                      const Divider(height: 24),
                    ],
                    if (offline.isNotEmpty) ...[
                      Padding(
                        padding:
                            const EdgeInsets.fromLTRB(14, 4, 14, 8),
                        child: Text(
                          'Offline (${offline.length})',
                          style: TextStyle(
                            fontSize: 12,
                            fontWeight: FontWeight.w600,
                            color: Colors.grey.shade500,
                            letterSpacing: 0.4,
                          ),
                        ),
                      ),
                      ...offline.map((peerId) {
                        return _memberTile(
                          context: context,
                          peerId: peerId,
                          nickname: _nicknameFor(peerId, profilesController),
                          online: false,
                          isSelf: false,
                          scheme: scheme,
                        );
                      }),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
