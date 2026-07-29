import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';

/// Shared `Contact` fake for unit and widget tests. `Contact` is a
/// `RustOpaqueInterface` marker — the native bridge is not initialized in the
/// test harness, so production callers cannot construct one.
class FakeContact implements Contact {
  FakeContact({required String id, required String contactNickname})
      : _id = id,
        _contactNickname = contactNickname;

  final String _id;
  final String _contactNickname;

  @override
  String id() => _id;

  @override
  PublicKey getPeerId() => FakePublicKey();

  @override
  bool idEq({required List<int> id}) => false;

  @override
  String? directConnectionString() => null;

  @override
  bool isDirect() => false;

  @override
  String nickname() => _contactNickname;

  @override
  double outputVolume() => 0.0;

  @override
  String peerId() => _id;

  @override
  Contact pubClone() => this;

  @override
  void setDirect({required bool isDirect}) {}

  @override
  void setDirectConnectionString({String? connectionString}) {}

  @override
  void setNickname({required String nickname}) {}

  @override
  void setOutputVolume({required double decibel}) {}

  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}

class FakePublicKey implements PublicKey {
  @override
  void dispose() {}

  @override
  bool get isDisposed => false;
}
