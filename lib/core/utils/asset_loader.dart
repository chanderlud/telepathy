import 'package:flutter/services.dart';
import 'package:telepathy/src/rust/telepathy.dart';

/// Reads the bytes of a sea file from the assets.
Future<List<int>> readSeaBytes(String assetName) {
  return readAssetBytes('sounds/$assetName.sea');
}

/// Reads the bytes of a file from the assets.
Future<List<int>> readAssetBytes(String assetName) async {
  final ByteData data = await rootBundle.load('assets/$assetName');
  final List<int> bytes = data.buffer.asUint8List();
  return bytes;
}

Future<void> updateDenoiseModel(String? model, Telepathy telepathy) async {
  if (model == null) {
    telepathy.setModel(model: null);
    return;
  }

  List<int> bytes = await readAssetBytes('models/$model.rnn');
  telepathy.setModel(model: Uint8List.fromList(bytes));
}

