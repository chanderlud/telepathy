import 'package:telepathy/models/room.dart';
import 'package:telepathy/src/rust/flutter.dart';

class Profile {
  final String id;
  final String nickname;
  final String peerId;
  final List<int> keypair;
  final Map<String, Contact> contacts;
  final Map<String, Room> rooms;

  Profile({
    required this.id,
    required this.nickname,
    required this.peerId,
    required this.keypair,
    required this.contacts,
    required this.rooms,
  });
}

