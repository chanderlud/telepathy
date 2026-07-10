import 'package:telepathy/core/rust/lib.dart';
import 'package:telepathy/core/rust/types.dart';

/// Shared `Contact` fake for unit and widget tests.
///
/// `Contact` is a `RustOpaqueInterface` marker — the native bridge is not
/// initialized in the test harness, so production callers cannot construct
/// one. Tests that need a `Contact` (to drive `StateController` or to build
/// `ContactWidget`) should instantiate this fake.
class FakeContact implements Contact {
  FakeContact({required String id, required String contactNickname})
      : _id = id,
        _contactNickname = contactNickname;

  final String _id;
  final String _contactNickname;

  @override
  String id() => _id;

  @override
  Future<PublicKey> getPeerId() async => FakePublicKey();

  @override
  bool idEq({required List<int> id}) => false;

  @override
  String nickname() => _contactNickname;

  @override
  double outputVolume() => 0.0;

  @override
  String peerId() => _id;

  @override
  Contact pubClone() => this;

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
