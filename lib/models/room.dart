import 'package:telepathy/src/rust/flutter.dart';

class Room {
  late String id;
  late List<String> peerIds;
  late String nickname;
  List<String> online = [];

  Room({
    required this.id,
    required this.peerIds,
    required this.nickname,
  });

  // Deserialize (from JSON)
  factory Room.fromJson(Map<String, dynamic> json) {
    List<String> peers = List<String>.from(json['peerIds']);

    return Room(
      id: roomHash(peers: peers),
      peerIds: peers,
      nickname: json['nickname'],
    );
  }

  // Serialize (to JSON)
  Map<String, dynamic> toJson() {
    return {
      'peerIds': peerIds,
      'nickname': nickname,
    };
  }
}
