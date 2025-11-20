import 'dart:convert';
import 'dart:typed_data';

import 'package:collection/collection.dart';
import 'package:telepathy/src/rust/error.dart';
import 'package:telepathy/src/rust/flutter.dart';
import 'package:flutter/cupertino.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:uuid/uuid.dart';
import 'package:telepathy/console.dart';

const defaultRelayAddress = '5.78.76.47:40142';
const defaultRelayId = '12D3KooWSePyyLtTHJsTMqMUS5pbQtdt8oJJX7rrEYCwG8bURYBG';
const defaultOverlayEnabled = false;
const defaultOverlayX = 0.0;
const defaultOverlayY = 0.0;
const defaultOverlayWidth = 600.0;
const defaultOverlayHeight = 38.0;
const defaultOverlayFontFamily = 'Inconsolata';
const defaultOverlayFontColor = 0xFFFFFFFF;
const defaultOverlayFontHeight = 36;
const defaultOverlayFontBackgroundColor = 0x80000000;

class SettingsController with ChangeNotifier {
  final FlutterSecureStorage storage;
  final SharedPreferences options;
  final List<String> args;

  SettingsController(
      {required this.storage, required this.options, required this.args});

  /// the ids of all available profiles
  late Map<String, Profile> profiles;

  /// the id of the active profile
  late String activeProfile;

  /// the output volume for calls (applies to output device)
  late double outputVolume;

  /// the input volume for calls (applies to input device)
  late double inputVolume;

  /// the output volume for sound effects
  late double soundVolume;

  /// the input sensitivity for calls
  late double inputSensitivity;

  /// whether to use rnnoise
  late bool useDenoise;

  /// the output device for calls
  late String? outputDevice;

  /// the input device for calls
  late String? inputDevice;

  /// whether to play custom ringtones
  late bool playCustomRingtones;

  /// the custom ringtone file
  late String? customRingtoneFile;

  /// the network configuration
  late NetworkConfig networkConfig;

  /// the screenshare configuration
  late ScreenshareConfig screenshareConfig;

  /// the overlay configuration
  late OverlayConfig overlayConfig;

  /// the codec configuration
  late CodecConfig codecConfig;

  /// the name of a denoise model
  late String? denoiseModel;

  late bool efficiencyMode;

  Map<String, Contact> get contacts => profiles[activeProfile]!.contacts;

  Map<String, Room> get rooms => profiles[activeProfile]!.rooms;

  List<int> get keypair => profiles[activeProfile]!.keypair;

  String get peerId => profiles[activeProfile]!.peerId;

  Future<void> init() async {
    // initialize an empty map for profiles
    profiles = {};
    // load a list of profile ids from the options storage
    List<String> profileIds = options.getStringList('profiles') ?? [];

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
      activeProfile = options.getString('activeProfile') ?? profiles.keys.first;
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

    // load the remaining options with default values as needed
    outputVolume = options.getDouble('outputVolume') ?? 0;
    inputVolume = options.getDouble('inputVolume') ?? 0;
    soundVolume = options.getDouble('soundVolume') ?? -10;
    inputSensitivity = options.getDouble('inputSensitivity') ?? -16;
    useDenoise = options.getBool('useDenoise') ?? true;
    outputDevice = options.getString('outputDevice');
    inputDevice = options.getString('inputDevice');
    playCustomRingtones = options.getBool('playCustomRingtones') ?? true;
    customRingtoneFile = options.getString('customRingtoneFile');
    denoiseModel = options.getString('denoiseModel');
    efficiencyMode = options.getBool('efficiencyMode') ?? false;

    networkConfig = loadNetworkConfig();
    screenshareConfig = await loadScreenshareConfig();
    overlayConfig = loadOverlayConfig();
    codecConfig = loadCodecConfig();

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

    // serialized contacts
    Map<String, Map<String, dynamic>> roomMap = {};

    for (MapEntry<String, Room> entry in rooms.entries) {
      roomMap[entry.key] = entry.value.toJson();
    }

    await storage.write(
      key: '$activeProfile-rooms',
      value: jsonEncode(roomMap),
    );
  }

  Future<void> updateOutputVolume(double volume) async {
    outputVolume = volume;
    await options.setDouble('outputVolume', volume);
    notifyListeners();
  }

  Future<void> updateInputVolume(double volume) async {
    inputVolume = volume;
    await options.setDouble('inputVolume', volume);
    notifyListeners();
  }

  Future<void> updateSoundVolume(double volume) async {
    soundVolume = volume;
    await options.setDouble('soundVolume', volume);
    notifyListeners();
  }

  Future<void> updateInputSensitivity(double sensitivity) async {
    inputSensitivity = sensitivity;
    await options.setDouble('inputSensitivity', sensitivity);
    notifyListeners();
  }

  Future<void> updateUseDenoise(bool use) async {
    useDenoise = use;
    await options.setBool('useDenoise', use);
    notifyListeners();
  }

  Future<void> updateOutputDevice(String? device) async {
    outputDevice = device;

    if (device != null) {
      await options.setString('outputDevice', device);
    } else {
      await options.remove('outputDevice');
    }

    notifyListeners();
  }

  Future<void> updateInputDevice(String? device) async {
    inputDevice = device;

    if (device != null) {
      await options.setString('inputDevice', device);
    } else {
      await options.remove('inputDevice');
    }

    notifyListeners();
  }

  Future<void> updatePlayCustomRingtones(bool play) async {
    playCustomRingtones = play;
    await options.setBool('playCustomRingtones', play);
    notifyListeners();
  }

  Future<void> updateEfficiencyMode(bool enabled) async {
    efficiencyMode = enabled;
    await options.setBool('efficiencyMode', enabled);
    notifyListeners();
  }

  Future<void> updateCustomRingtoneFile(String? file) async {
    customRingtoneFile = file;

    if (file != null) {
      await options.setString('customRingtoneFile', file);
    } else {
      await options.remove('customRingtoneFile');
    }

    notifyListeners();
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
    await storage.delete(key: '$id-nickname');

    if (activeProfile == id) {
      await setActiveProfile(profiles.keys.first);
    } else {
      notifyListeners();
    }
  }

  Future<void> setActiveProfile(String id) async {
    activeProfile = id;
    await options.setString('activeProfile', id);
    notifyListeners();
  }

  Future<void> setDenoiseModel(String? model) async {
    denoiseModel = model;

    if (model != null) {
      await options.setString('denoiseModel', model);
    } else {
      await options.remove('denoiseModel');
    }

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

  NetworkConfig loadNetworkConfig() {
    try {
      return NetworkConfig(
        relayAddress: options.getString('relayAddress') ?? defaultRelayAddress,
        relayId: options.getString('relayId') ?? defaultRelayId,
      );
    } on DartError catch (e) {
      DebugConsole.warn('invalid network config values: $e');
      return NetworkConfig(
          relayAddress: defaultRelayAddress, relayId: defaultRelayId);
    }
  }

  Future<void> saveNetworkConfig() async {
    await options.setString(
        'relayAddress', await networkConfig.getRelayAddress());
    await options.setString('relayId', await networkConfig.getRelayId());
  }

  Future<ScreenshareConfig> loadScreenshareConfig() async {
    return await ScreenshareConfig.newInstance(
      configStr: options.getString('screenshareConfig') ?? '',
    );
  }

  Future<void> saveScreenshareConfig() async {
    await options.setString('screenshareConfig', screenshareConfig.toString());
  }

  CodecConfig loadCodecConfig() {
    return CodecConfig(
      enabled: options.getBool('codecEnabled') ?? true,
      vbr: options.getBool('codecVbr') ?? true,
      residualBits: options.getDouble('codecResidualBits') ?? 5.0,
    );
  }

  Future<void> saveCodecConfig() async {
    (bool, bool, double) values = codecConfig.toValues();
    await options.setBool('codecEnabled', values.$1);
    await options.setBool('codecVbr', values.$2);
    await options.setDouble('codecResidualBits', values.$3);
  }

  OverlayConfig loadOverlayConfig() {
    try {
      return OverlayConfig(
        enabled: options.getBool('overlayEnabled') ?? defaultOverlayEnabled,
        x: options.getDouble('overlayX') ?? defaultOverlayX,
        y: options.getDouble('overlayY') ?? defaultOverlayY,
        width: options.getDouble('overlayWidth') ?? defaultOverlayWidth,
        height: options.getDouble('overlayHeight') ?? defaultOverlayHeight,
        fontFamily:
            options.getString('overlayFontFamily') ?? defaultOverlayFontFamily,
        fontColor: Color(
            options.getInt('overlayFontColor') ?? defaultOverlayFontColor),
        fontHeight:
            options.getInt('overlayFontHeight') ?? defaultOverlayFontHeight,
        backgroundColor: Color(options.getInt('overlayBackgroundColor') ??
            defaultOverlayFontBackgroundColor),
      );
    } on DartError catch (e) {
      DebugConsole.warn('invalid overlay config format: $e');

      return OverlayConfig(
        enabled: defaultOverlayEnabled,
        x: defaultOverlayX,
        y: defaultOverlayY,
        width: defaultOverlayWidth,
        height: defaultOverlayHeight,
        fontFamily: defaultOverlayFontFamily,
        fontColor: const Color(defaultOverlayFontColor),
        fontHeight: defaultOverlayFontHeight,
        backgroundColor: const Color(defaultOverlayFontBackgroundColor),
      );
    }
  }

  Future<void> saveOverlayConfig() async {
    await options.setBool('overlayEnabled', overlayConfig.enabled);
    await options.setDouble('overlayX', overlayConfig.x);
    await options.setDouble('overlayY', overlayConfig.y);
    await options.setDouble('overlayWidth', overlayConfig.width);
    await options.setDouble('overlayHeight', overlayConfig.height);
    await options.setString('overlayFontFamily', overlayConfig.fontFamily);
    await options.setInt(
        'overlayFontColor', overlayConfig.fontColor.toARGB32());
    await options.setInt('overlayFontHeight', overlayConfig.fontHeight);
    await options.setInt(
        'overlayBackgroundColor', overlayConfig.backgroundColor.toARGB32());
  }
}

class InterfaceController with ChangeNotifier {
  final SharedPreferences options;

  InterfaceController({required this.options});

  late int primaryColor;

  int get secondaryColor => darkenColor(primaryColor, 0.1);

  Future<void> init() async {
    primaryColor = options.getInt("primaryColor") ?? 0xFF5538e5;
    notifyListeners();
  }

  Future<void> setPrimaryColor(int color) async {
    primaryColor = color;
    await options.setInt('primaryColor', color);
    notifyListeners();
  }
}

class OverlayConfig {
  bool enabled;
  double x;
  double y;
  double width;
  double height;
  String fontFamily;
  Color fontColor;
  int fontHeight;
  Color backgroundColor;

  OverlayConfig({
    required this.enabled,
    required this.x,
    required this.y,
    required this.width,
    required this.height,
    required this.fontFamily,
    required this.fontColor,
    required this.fontHeight,
    required this.backgroundColor,
  });
}

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

int darkenColor(int colorInt, double amount) {
  assert(amount >= 0 && amount <= 1, '"amount" should be between 0.0 and 1.0');

  // Separate out ARGB channels:
  final a = (colorInt >> 24) & 0xFF;
  final r = (colorInt >> 16) & 0xFF;
  final g = (colorInt >> 8) & 0xFF;
  final b = colorInt & 0xFF;

  // Multiply each channel by (1 - amount) to darken; clamp to [0,255].
  int darkerR = (r * (1 - amount)).clamp(0, 255).toInt();
  int darkerG = (g * (1 - amount)).clamp(0, 255).toInt();
  int darkerB = (b * (1 - amount)).clamp(0, 255).toInt();

  // Reassemble back into a 32-bit ARGB int:
  return (a << 24) | (darkerR << 16) | (darkerG << 8) | darkerB;
}
