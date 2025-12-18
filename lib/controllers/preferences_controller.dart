import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

class PreferencesController with ChangeNotifier {
  final SharedPreferencesAsync options;

  /// whether to play custom ringtones
  late bool playCustomRingtones;

  /// the custom ringtone file
  late String? customRingtoneFile;

  late bool efficiencyMode;

  PreferencesController({required this.options});

  Future<void> init() async {
    playCustomRingtones = await options.getBool('playCustomRingtones') ?? true;
    customRingtoneFile = await options.getString('customRingtoneFile');
    efficiencyMode = await options.getBool('efficiencyMode') ?? false;
    notifyListeners();
  }

  Future<void> updatePlayCustomRingtones(bool play) async {
    playCustomRingtones = play;
    await options.setBool('playCustomRingtones', play);
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

  Future<void> updateEfficiencyMode(bool enabled) async {
    efficiencyMode = enabled;
    await options.setBool('efficiencyMode', enabled);
    notifyListeners();
  }
}
