import 'dart:convert';

/// Serializes room details into a shareable format (JSON).
///
/// Format:
/// {
///   "nickname": "Room Name",
///   "peerIds": ["peer1", "peer2"]
/// }
String serializeRoomDetails(String nickname, List<String> peerIds) {
  return jsonEncode(<String, Object?>{
    'nickname': nickname,
    'peerIds': peerIds,
  });
}

/// Parses room details from clipboard text.
///
/// Returns `null` if the data is not in the expected JSON format.
({String nickname, List<String> peerIds})? parseRoomDetails(String data) {
  try {
    final decoded = jsonDecode(data.trim());
    if (decoded is! Map) return null;

    final nickname = decoded['nickname'];
    final peerIds = decoded['peerIds'];

    if (nickname is! String) return null;
    if (peerIds is! List) return null;

    final parsedPeerIds = <String>[];
    for (final p in peerIds) {
      if (p is! String) return null;
      final trimmed = p.trim();
      if (trimmed.isEmpty) return null;
      parsedPeerIds.add(trimmed);
    }

    final trimmedNickname = nickname.trim();
    if (trimmedNickname.isEmpty) return null;
    if (parsedPeerIds.isEmpty) return null;

    return (nickname: trimmedNickname, peerIds: parsedPeerIds);
  } catch (_) {
    return null;
  }
}
