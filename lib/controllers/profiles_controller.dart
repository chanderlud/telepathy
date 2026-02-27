import 'dart:convert';

import 'package:collection/collection.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:uuid/uuid.dart';

class ProfilesController with ChangeNotifier {
  final FlutterSecureStorage storage;
  final SharedPreferencesAsync options;

  ProfilesController({required this.storage, required this.options});

  /// the ids of all available profiles
  late Map<String, Profile> profiles;

  /// the id of the active profile
  late String activeProfile;

  Map<String, Contact> get contacts => profiles[activeProfile]!.contacts;

  Map<String, Room> get rooms => profiles[activeProfile]!.rooms;

  List<int> get keypair => profiles[activeProfile]!.keypair;

  String get peerId => profiles[activeProfile]!.peerId;

  Future<void> init(List<String> args) async {
    // initialize an empty map for profiles
    profiles = {};
    // load a list of profile ids from the options storage
    List<String> profileIds = await options.getStringList('profiles') ?? [];

    // load each profile from the secure storage
    for (String id in profileIds) {
      String? keyStr = await storage.read(key: '$id-keypair');
      String? peerId = await storage.read(key: '$id-peerId');

      // if the key is missing, skip this profile
      if (keyStr == null || peerId == null) {
        await removeProfile(id);
        continue;
      }

      // load the contacts for this profile
      Map<String, Contact> contacts = await loadContacts(id);
      Map<String, Room> rooms = await loadRooms(id);
      String nickname =
          await storage.read(key: '$id-nickname') ?? 'Unnamed Profile';
      List<int> keyBytes = base64Decode(keyStr);

      // construct the profile object and add it to the profiles map
      profiles[id] = Profile(
        id: id,
        nickname: nickname,
        peerId: peerId,
        keypair: keyBytes,
        contacts: contacts,
        rooms: rooms,
      );
    }

    if (profiles.isEmpty) {
      // if there are no profiles, create a default profile
      activeProfile = await createProfile('Default');
    } else {
      // if there are profiles, load the active profile or use the first profile if needed
      activeProfile =
          await options.getString('activeProfile') ?? profiles.keys.first;
    }

    String? override = args.elementAtOrNull(0);
    if (override != null) {
      String? profileId = profiles.entries
          .firstWhereOrNull((c) => c.value.nickname == override)
          ?.key;
      if (profileId != null) {
        activeProfile = profileId;
      }
    }

    // ensure that the active profile is now persisted
    await setActiveProfile(activeProfile);

    notifyListeners();
  }

  /// This function can raise [DartError] if the verifying key is invalid
  Contact addContact(
    String nickname,
    String peerId,
  ) {
    Contact contact = Contact(nickname: nickname, peerId: peerId);
    contacts[contact.id()] = contact;

    saveContacts();
    return contact;
  }

  Contact? getContact(String id) {
    return contacts[id];
  }

  void removeContact(Contact contact) {
    contacts.remove(contact.id());
    saveContacts();
  }

  /// Saves the contacts for activeProfile
  Future<void> saveContacts() async {
    // notify listeners right away because the contacts are already updated
    notifyListeners();

    // serialized contacts
    Map<String, Map<String, dynamic>> contactsMap = {};

    for (MapEntry<String, Contact> entry in contacts.entries) {
      Map<String, dynamic> contact = {};
      contact['nickname'] = entry.value.nickname();
      contact['peerId'] = entry.value.peerId();
      contactsMap[entry.key] = contact;
    }

    await storage.write(
      key: '$activeProfile-contacts',
      value: jsonEncode(contactsMap),
    );
  }

  Room addRoom(
    String nickname,
    List<String> peerIds,
  ) {
    Room room = Room(
        id: roomHash(peers: peerIds), peerIds: peerIds, nickname: nickname);
    rooms[room.id] = room;

    saveRooms();
    return room;
  }

  Future<void> saveRooms() async {
    // notify listeners right away because the rooms are already updated
    notifyListeners();

    // serialized rooms
    Map<String, Map<String, dynamic>> roomMap = {};

    for (MapEntry<String, Room> entry in rooms.entries) {
      roomMap[entry.key] = entry.value.toJson();
    }

    await storage.write(
      key: '$activeProfile-rooms',
      value: jsonEncode(roomMap),
    );
  }

  Future<String> createProfile(String nickname) async {
    String peerId;
    Uint8List keypair;

    (peerId, keypair) = generateKeys();
    String id = const Uuid().v4();

    await storage.write(key: '$id-keypair', value: base64Encode(keypair));
    await storage.write(key: '$id-peerId', value: peerId);
    await storage.write(key: '$id-contacts', value: jsonEncode({}));
    await storage.write(key: '$id-rooms', value: jsonEncode({}));
    await storage.write(key: '$id-nickname', value: nickname);

    profiles[id] = Profile(
      id: id,
      nickname: nickname,
      peerId: peerId,
      keypair: keypair,
      contacts: {},
      rooms: {},
    );

    await options.setStringList('profiles', profiles.keys.toList());
    notifyListeners();

    return id;
  }

  Future<void> removeProfile(String id) async {
    profiles.remove(id);
    await options.setStringList('profiles', profiles.keys.toList());

    await storage.delete(key: '$id-keypair');
    await storage.delete(key: '$id-peerId');
    await storage.delete(key: '$id-contacts');
    await storage.delete(key: '$id-rooms');
    await storage.delete(key: '$id-nickname');

    if (activeProfile == id) {
      if (profiles.isNotEmpty) {
        await setActiveProfile(profiles.keys.first);
      } else {
        activeProfile = await createProfile('Default');
        await setActiveProfile(activeProfile);
      }
    } else {
      notifyListeners();
    }
  }

  Future<void> setActiveProfile(String id) async {
    activeProfile = id;
    await options.setString('activeProfile', id);
    notifyListeners();
  }

  Future<Map<String, Contact>> loadContacts(String id) async {
    Map<String, Contact> contacts = {};
    String? contactsStr = await storage.read(key: '$id-contacts');

    if (contactsStr != null) {
      Map<String, dynamic> contactsMap = jsonDecode(contactsStr);
      contactsMap.forEach((id, value) {
        String nickname = value['nickname'];
        String peerId = value['peerId'];

        try {
          contacts[id] =
              Contact.fromParts(id: id, nickname: nickname, peerId: peerId);
        } on DartError catch (e) {
          DebugConsole.warn('invalid contact format: $e');
          return;
        }
      });
    }

    return contacts;
  }

  Future<Map<String, Room>> loadRooms(String id) async {
    Map<String, Room> rooms = {};
    String? roomStr = await storage.read(key: '$id-rooms');

    if (roomStr != null) {
      Map<String, dynamic> roomMap = jsonDecode(roomStr);
      roomMap.forEach((id, value) {
        rooms[id] = Room.fromJson(value);
      });
    }

    return rooms;
  }
}
