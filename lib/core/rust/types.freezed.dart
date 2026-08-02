// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'types.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// dart format off
T _$identity<T>(T value) => value;

/// @nodoc
mixin _$CallState {
  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is CallState);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'CallState()';
  }
}

/// @nodoc
class $CallStateCopyWith<$Res> {
  $CallStateCopyWith(CallState _, $Res Function(CallState) __);
}

/// Adds pattern-matching-related methods to [CallState].
extension CallStatePatterns on CallState {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(CallState_Connected value)? connected,
    TResult Function(CallState_Waiting value)? waiting,
    TResult Function(CallState_RoomJoin value)? roomJoin,
    TResult Function(CallState_RoomLeave value)? roomLeave,
    TResult Function(CallState_CallEnded value)? callEnded,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected() when connected != null:
        return connected(_that);
      case CallState_Waiting() when waiting != null:
        return waiting(_that);
      case CallState_RoomJoin() when roomJoin != null:
        return roomJoin(_that);
      case CallState_RoomLeave() when roomLeave != null:
        return roomLeave(_that);
      case CallState_CallEnded() when callEnded != null:
        return callEnded(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(CallState_Connected value) connected,
    required TResult Function(CallState_Waiting value) waiting,
    required TResult Function(CallState_RoomJoin value) roomJoin,
    required TResult Function(CallState_RoomLeave value) roomLeave,
    required TResult Function(CallState_CallEnded value) callEnded,
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected():
        return connected(_that);
      case CallState_Waiting():
        return waiting(_that);
      case CallState_RoomJoin():
        return roomJoin(_that);
      case CallState_RoomLeave():
        return roomLeave(_that);
      case CallState_CallEnded():
        return callEnded(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(CallState_Connected value)? connected,
    TResult? Function(CallState_Waiting value)? waiting,
    TResult? Function(CallState_RoomJoin value)? roomJoin,
    TResult? Function(CallState_RoomLeave value)? roomLeave,
    TResult? Function(CallState_CallEnded value)? callEnded,
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected() when connected != null:
        return connected(_that);
      case CallState_Waiting() when waiting != null:
        return waiting(_that);
      case CallState_RoomJoin() when roomJoin != null:
        return roomJoin(_that);
      case CallState_RoomLeave() when roomLeave != null:
        return roomLeave(_that);
      case CallState_CallEnded() when callEnded != null:
        return callEnded(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function()? connected,
    TResult Function()? waiting,
    TResult Function(String field0)? roomJoin,
    TResult Function(String field0)? roomLeave,
    TResult Function(String field0, bool field1)? callEnded,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected() when connected != null:
        return connected();
      case CallState_Waiting() when waiting != null:
        return waiting();
      case CallState_RoomJoin() when roomJoin != null:
        return roomJoin(_that.field0);
      case CallState_RoomLeave() when roomLeave != null:
        return roomLeave(_that.field0);
      case CallState_CallEnded() when callEnded != null:
        return callEnded(_that.field0, _that.field1);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function() connected,
    required TResult Function() waiting,
    required TResult Function(String field0) roomJoin,
    required TResult Function(String field0) roomLeave,
    required TResult Function(String field0, bool field1) callEnded,
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected():
        return connected();
      case CallState_Waiting():
        return waiting();
      case CallState_RoomJoin():
        return roomJoin(_that.field0);
      case CallState_RoomLeave():
        return roomLeave(_that.field0);
      case CallState_CallEnded():
        return callEnded(_that.field0, _that.field1);
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function()? connected,
    TResult? Function()? waiting,
    TResult? Function(String field0)? roomJoin,
    TResult? Function(String field0)? roomLeave,
    TResult? Function(String field0, bool field1)? callEnded,
  }) {
    final _that = this;
    switch (_that) {
      case CallState_Connected() when connected != null:
        return connected();
      case CallState_Waiting() when waiting != null:
        return waiting();
      case CallState_RoomJoin() when roomJoin != null:
        return roomJoin(_that.field0);
      case CallState_RoomLeave() when roomLeave != null:
        return roomLeave(_that.field0);
      case CallState_CallEnded() when callEnded != null:
        return callEnded(_that.field0, _that.field1);
      case _:
        return null;
    }
  }
}

/// @nodoc

class CallState_Connected extends CallState {
  const CallState_Connected() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is CallState_Connected);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'CallState.connected()';
  }
}

/// @nodoc

class CallState_Waiting extends CallState {
  const CallState_Waiting() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is CallState_Waiting);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'CallState.waiting()';
  }
}

/// @nodoc

class CallState_RoomJoin extends CallState {
  const CallState_RoomJoin(this.field0) : super._();

  final String field0;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $CallState_RoomJoinCopyWith<CallState_RoomJoin> get copyWith =>
      _$CallState_RoomJoinCopyWithImpl<CallState_RoomJoin>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is CallState_RoomJoin &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'CallState.roomJoin(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $CallState_RoomJoinCopyWith<$Res>
    implements $CallStateCopyWith<$Res> {
  factory $CallState_RoomJoinCopyWith(
          CallState_RoomJoin value, $Res Function(CallState_RoomJoin) _then) =
      _$CallState_RoomJoinCopyWithImpl;
  @useResult
  $Res call({String field0});
}

/// @nodoc
class _$CallState_RoomJoinCopyWithImpl<$Res>
    implements $CallState_RoomJoinCopyWith<$Res> {
  _$CallState_RoomJoinCopyWithImpl(this._self, this._then);

  final CallState_RoomJoin _self;
  final $Res Function(CallState_RoomJoin) _then;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(CallState_RoomJoin(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as String,
    ));
  }
}

/// @nodoc

class CallState_RoomLeave extends CallState {
  const CallState_RoomLeave(this.field0) : super._();

  final String field0;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $CallState_RoomLeaveCopyWith<CallState_RoomLeave> get copyWith =>
      _$CallState_RoomLeaveCopyWithImpl<CallState_RoomLeave>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is CallState_RoomLeave &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'CallState.roomLeave(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $CallState_RoomLeaveCopyWith<$Res>
    implements $CallStateCopyWith<$Res> {
  factory $CallState_RoomLeaveCopyWith(
          CallState_RoomLeave value, $Res Function(CallState_RoomLeave) _then) =
      _$CallState_RoomLeaveCopyWithImpl;
  @useResult
  $Res call({String field0});
}

/// @nodoc
class _$CallState_RoomLeaveCopyWithImpl<$Res>
    implements $CallState_RoomLeaveCopyWith<$Res> {
  _$CallState_RoomLeaveCopyWithImpl(this._self, this._then);

  final CallState_RoomLeave _self;
  final $Res Function(CallState_RoomLeave) _then;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(CallState_RoomLeave(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as String,
    ));
  }
}

/// @nodoc

class CallState_CallEnded extends CallState {
  const CallState_CallEnded(this.field0, this.field1) : super._();

  final String field0;
  final bool field1;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $CallState_CallEndedCopyWith<CallState_CallEnded> get copyWith =>
      _$CallState_CallEndedCopyWithImpl<CallState_CallEnded>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is CallState_CallEnded &&
            (identical(other.field0, field0) || other.field0 == field0) &&
            (identical(other.field1, field1) || other.field1 == field1));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0, field1);

  @override
  String toString() {
    return 'CallState.callEnded(field0: $field0, field1: $field1)';
  }
}

/// @nodoc
abstract mixin class $CallState_CallEndedCopyWith<$Res>
    implements $CallStateCopyWith<$Res> {
  factory $CallState_CallEndedCopyWith(
          CallState_CallEnded value, $Res Function(CallState_CallEnded) _then) =
      _$CallState_CallEndedCopyWithImpl;
  @useResult
  $Res call({String field0, bool field1});
}

/// @nodoc
class _$CallState_CallEndedCopyWithImpl<$Res>
    implements $CallState_CallEndedCopyWith<$Res> {
  _$CallState_CallEndedCopyWithImpl(this._self, this._then);

  final CallState_CallEnded _self;
  final $Res Function(CallState_CallEnded) _then;

  /// Create a copy of CallState
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
    Object? field1 = null,
  }) {
    return _then(CallState_CallEnded(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as String,
      null == field1
          ? _self.field1
          : field1 // ignore: cast_nullable_to_non_nullable
              as bool,
    ));
  }
}

/// @nodoc
mixin _$SessionStatus {
  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is SessionStatus);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'SessionStatus()';
  }
}

/// @nodoc
class $SessionStatusCopyWith<$Res> {
  $SessionStatusCopyWith(SessionStatus _, $Res Function(SessionStatus) __);
}

/// Adds pattern-matching-related methods to [SessionStatus].
extension SessionStatusPatterns on SessionStatus {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(SessionStatus_Connecting value)? connecting,
    TResult Function(SessionStatus_Connected value)? connected,
    TResult Function(SessionStatus_Inactive value)? inactive,
    TResult Function(SessionStatus_Unknown value)? unknown,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting(_that);
      case SessionStatus_Connected() when connected != null:
        return connected(_that);
      case SessionStatus_Inactive() when inactive != null:
        return inactive(_that);
      case SessionStatus_Unknown() when unknown != null:
        return unknown(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(SessionStatus_Connecting value) connecting,
    required TResult Function(SessionStatus_Connected value) connected,
    required TResult Function(SessionStatus_Inactive value) inactive,
    required TResult Function(SessionStatus_Unknown value) unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting():
        return connecting(_that);
      case SessionStatus_Connected():
        return connected(_that);
      case SessionStatus_Inactive():
        return inactive(_that);
      case SessionStatus_Unknown():
        return unknown(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(SessionStatus_Connecting value)? connecting,
    TResult? Function(SessionStatus_Connected value)? connected,
    TResult? Function(SessionStatus_Inactive value)? inactive,
    TResult? Function(SessionStatus_Unknown value)? unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting(_that);
      case SessionStatus_Connected() when connected != null:
        return connected(_that);
      case SessionStatus_Inactive() when inactive != null:
        return inactive(_that);
      case SessionStatus_Unknown() when unknown != null:
        return unknown(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function()? connecting,
    TResult Function(bool relayed, String remoteAddress)? connected,
    TResult Function()? inactive,
    TResult Function()? unknown,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting();
      case SessionStatus_Connected() when connected != null:
        return connected(_that.relayed, _that.remoteAddress);
      case SessionStatus_Inactive() when inactive != null:
        return inactive();
      case SessionStatus_Unknown() when unknown != null:
        return unknown();
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function() connecting,
    required TResult Function(bool relayed, String remoteAddress) connected,
    required TResult Function() inactive,
    required TResult Function() unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting():
        return connecting();
      case SessionStatus_Connected():
        return connected(_that.relayed, _that.remoteAddress);
      case SessionStatus_Inactive():
        return inactive();
      case SessionStatus_Unknown():
        return unknown();
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function()? connecting,
    TResult? Function(bool relayed, String remoteAddress)? connected,
    TResult? Function()? inactive,
    TResult? Function()? unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting();
      case SessionStatus_Connected() when connected != null:
        return connected(_that.relayed, _that.remoteAddress);
      case SessionStatus_Inactive() when inactive != null:
        return inactive();
      case SessionStatus_Unknown() when unknown != null:
        return unknown();
      case _:
        return null;
    }
  }
}

/// @nodoc

class SessionStatus_Connecting extends SessionStatus {
  const SessionStatus_Connecting() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is SessionStatus_Connecting);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'SessionStatus.connecting()';
  }
}

/// @nodoc

class SessionStatus_Connected extends SessionStatus {
  const SessionStatus_Connected(
      {required this.relayed, required this.remoteAddress})
      : super._();

  final bool relayed;
  final String remoteAddress;

  /// Create a copy of SessionStatus
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $SessionStatus_ConnectedCopyWith<SessionStatus_Connected> get copyWith =>
      _$SessionStatus_ConnectedCopyWithImpl<SessionStatus_Connected>(
          this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is SessionStatus_Connected &&
            (identical(other.relayed, relayed) || other.relayed == relayed) &&
            (identical(other.remoteAddress, remoteAddress) ||
                other.remoteAddress == remoteAddress));
  }

  @override
  int get hashCode => Object.hash(runtimeType, relayed, remoteAddress);

  @override
  String toString() {
    return 'SessionStatus.connected(relayed: $relayed, remoteAddress: $remoteAddress)';
  }
}

/// @nodoc
abstract mixin class $SessionStatus_ConnectedCopyWith<$Res>
    implements $SessionStatusCopyWith<$Res> {
  factory $SessionStatus_ConnectedCopyWith(SessionStatus_Connected value,
          $Res Function(SessionStatus_Connected) _then) =
      _$SessionStatus_ConnectedCopyWithImpl;
  @useResult
  $Res call({bool relayed, String remoteAddress});
}

/// @nodoc
class _$SessionStatus_ConnectedCopyWithImpl<$Res>
    implements $SessionStatus_ConnectedCopyWith<$Res> {
  _$SessionStatus_ConnectedCopyWithImpl(this._self, this._then);

  final SessionStatus_Connected _self;
  final $Res Function(SessionStatus_Connected) _then;

  /// Create a copy of SessionStatus
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? relayed = null,
    Object? remoteAddress = null,
  }) {
    return _then(SessionStatus_Connected(
      relayed: null == relayed
          ? _self.relayed
          : relayed // ignore: cast_nullable_to_non_nullable
              as bool,
      remoteAddress: null == remoteAddress
          ? _self.remoteAddress
          : remoteAddress // ignore: cast_nullable_to_non_nullable
              as String,
    ));
  }
}

/// @nodoc

class SessionStatus_Inactive extends SessionStatus {
  const SessionStatus_Inactive() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is SessionStatus_Inactive);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'SessionStatus.inactive()';
  }
}

/// @nodoc

class SessionStatus_Unknown extends SessionStatus {
  const SessionStatus_Unknown() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is SessionStatus_Unknown);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'SessionStatus.unknown()';
  }
}

/// @nodoc
mixin _$VideoCapabilityAvailability {
  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoCapabilityAvailability);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoCapabilityAvailability()';
  }
}

/// @nodoc
class $VideoCapabilityAvailabilityCopyWith<$Res> {
  $VideoCapabilityAvailabilityCopyWith(VideoCapabilityAvailability _,
      $Res Function(VideoCapabilityAvailability) __);
}

/// Adds pattern-matching-related methods to [VideoCapabilityAvailability].
extension VideoCapabilityAvailabilityPatterns on VideoCapabilityAvailability {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(VideoCapabilityAvailability_Available value)? available,
    TResult Function(VideoCapabilityAvailability_Unavailable value)?
        unavailable,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available() when available != null:
        return available(_that);
      case VideoCapabilityAvailability_Unavailable() when unavailable != null:
        return unavailable(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(VideoCapabilityAvailability_Available value)
        available,
    required TResult Function(VideoCapabilityAvailability_Unavailable value)
        unavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available():
        return available(_that);
      case VideoCapabilityAvailability_Unavailable():
        return unavailable(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(VideoCapabilityAvailability_Available value)? available,
    TResult? Function(VideoCapabilityAvailability_Unavailable value)?
        unavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available() when available != null:
        return available(_that);
      case VideoCapabilityAvailability_Unavailable() when unavailable != null:
        return unavailable(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function()? available,
    TResult Function(VideoUnavailable field0)? unavailable,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available() when available != null:
        return available();
      case VideoCapabilityAvailability_Unavailable() when unavailable != null:
        return unavailable(_that.field0);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function() available,
    required TResult Function(VideoUnavailable field0) unavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available():
        return available();
      case VideoCapabilityAvailability_Unavailable():
        return unavailable(_that.field0);
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function()? available,
    TResult? Function(VideoUnavailable field0)? unavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoCapabilityAvailability_Available() when available != null:
        return available();
      case VideoCapabilityAvailability_Unavailable() when unavailable != null:
        return unavailable(_that.field0);
      case _:
        return null;
    }
  }
}

/// @nodoc

class VideoCapabilityAvailability_Available
    extends VideoCapabilityAvailability {
  const VideoCapabilityAvailability_Available() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoCapabilityAvailability_Available);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoCapabilityAvailability.available()';
  }
}

/// @nodoc

class VideoCapabilityAvailability_Unavailable
    extends VideoCapabilityAvailability {
  const VideoCapabilityAvailability_Unavailable(this.field0) : super._();

  final VideoUnavailable field0;

  /// Create a copy of VideoCapabilityAvailability
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoCapabilityAvailability_UnavailableCopyWith<
          VideoCapabilityAvailability_Unavailable>
      get copyWith => _$VideoCapabilityAvailability_UnavailableCopyWithImpl<
          VideoCapabilityAvailability_Unavailable>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoCapabilityAvailability_Unavailable &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoCapabilityAvailability.unavailable(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoCapabilityAvailability_UnavailableCopyWith<$Res>
    implements $VideoCapabilityAvailabilityCopyWith<$Res> {
  factory $VideoCapabilityAvailability_UnavailableCopyWith(
          VideoCapabilityAvailability_Unavailable value,
          $Res Function(VideoCapabilityAvailability_Unavailable) _then) =
      _$VideoCapabilityAvailability_UnavailableCopyWithImpl;
  @useResult
  $Res call({VideoUnavailable field0});

  $VideoUnavailableCopyWith<$Res> get field0;
}

/// @nodoc
class _$VideoCapabilityAvailability_UnavailableCopyWithImpl<$Res>
    implements $VideoCapabilityAvailability_UnavailableCopyWith<$Res> {
  _$VideoCapabilityAvailability_UnavailableCopyWithImpl(this._self, this._then);

  final VideoCapabilityAvailability_Unavailable _self;
  final $Res Function(VideoCapabilityAvailability_Unavailable) _then;

  /// Create a copy of VideoCapabilityAvailability
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoCapabilityAvailability_Unavailable(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoUnavailable,
    ));
  }

  /// Create a copy of VideoCapabilityAvailability
  /// with the given fields replaced by the non-null parameter values.
  @override
  @pragma('vm:prefer-inline')
  $VideoUnavailableCopyWith<$Res> get field0 {
    return $VideoUnavailableCopyWith<$Res>(_self.field0, (value) {
      return _then(_self.copyWith(field0: value));
    });
  }
}

/// @nodoc
mixin _$VideoMediaFormat {
  VideoCodec get field0;

  /// Create a copy of VideoMediaFormat
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoMediaFormatCopyWith<VideoMediaFormat> get copyWith =>
      _$VideoMediaFormatCopyWithImpl<VideoMediaFormat>(
          this as VideoMediaFormat, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoMediaFormat &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoMediaFormat(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoMediaFormatCopyWith<$Res> {
  factory $VideoMediaFormatCopyWith(
          VideoMediaFormat value, $Res Function(VideoMediaFormat) _then) =
      _$VideoMediaFormatCopyWithImpl;
  @useResult
  $Res call({VideoCodec field0});
}

/// @nodoc
class _$VideoMediaFormatCopyWithImpl<$Res>
    implements $VideoMediaFormatCopyWith<$Res> {
  _$VideoMediaFormatCopyWithImpl(this._self, this._then);

  final VideoMediaFormat _self;
  final $Res Function(VideoMediaFormat) _then;

  /// Create a copy of VideoMediaFormat
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  @override
  $Res call({
    Object? field0 = null,
  }) {
    return _then(_self.copyWith(
      field0: null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoCodec,
    ));
  }
}

/// Adds pattern-matching-related methods to [VideoMediaFormat].
extension VideoMediaFormatPatterns on VideoMediaFormat {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(VideoMediaFormat_MpegTs value)? mpegTs,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs() when mpegTs != null:
        return mpegTs(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(VideoMediaFormat_MpegTs value) mpegTs,
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs():
        return mpegTs(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(VideoMediaFormat_MpegTs value)? mpegTs,
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs() when mpegTs != null:
        return mpegTs(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function(VideoCodec field0)? mpegTs,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs() when mpegTs != null:
        return mpegTs(_that.field0);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function(VideoCodec field0) mpegTs,
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs():
        return mpegTs(_that.field0);
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function(VideoCodec field0)? mpegTs,
  }) {
    final _that = this;
    switch (_that) {
      case VideoMediaFormat_MpegTs() when mpegTs != null:
        return mpegTs(_that.field0);
      case _:
        return null;
    }
  }
}

/// @nodoc

class VideoMediaFormat_MpegTs extends VideoMediaFormat {
  const VideoMediaFormat_MpegTs(this.field0) : super._();

  @override
  final VideoCodec field0;

  /// Create a copy of VideoMediaFormat
  /// with the given fields replaced by the non-null parameter values.
  @override
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoMediaFormat_MpegTsCopyWith<VideoMediaFormat_MpegTs> get copyWith =>
      _$VideoMediaFormat_MpegTsCopyWithImpl<VideoMediaFormat_MpegTs>(
          this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoMediaFormat_MpegTs &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoMediaFormat.mpegTs(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoMediaFormat_MpegTsCopyWith<$Res>
    implements $VideoMediaFormatCopyWith<$Res> {
  factory $VideoMediaFormat_MpegTsCopyWith(VideoMediaFormat_MpegTs value,
          $Res Function(VideoMediaFormat_MpegTs) _then) =
      _$VideoMediaFormat_MpegTsCopyWithImpl;
  @override
  @useResult
  $Res call({VideoCodec field0});
}

/// @nodoc
class _$VideoMediaFormat_MpegTsCopyWithImpl<$Res>
    implements $VideoMediaFormat_MpegTsCopyWith<$Res> {
  _$VideoMediaFormat_MpegTsCopyWithImpl(this._self, this._then);

  final VideoMediaFormat_MpegTs _self;
  final $Res Function(VideoMediaFormat_MpegTs) _then;

  /// Create a copy of VideoMediaFormat
  /// with the given fields replaced by the non-null parameter values.
  @override
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoMediaFormat_MpegTs(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoCodec,
    ));
  }
}

/// @nodoc
mixin _$VideoStartOutcome {
  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is VideoStartOutcome);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoStartOutcome()';
  }
}

/// @nodoc
class $VideoStartOutcomeCopyWith<$Res> {
  $VideoStartOutcomeCopyWith(
      VideoStartOutcome _, $Res Function(VideoStartOutcome) __);
}

/// Adds pattern-matching-related methods to [VideoStartOutcome].
extension VideoStartOutcomePatterns on VideoStartOutcome {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(VideoStartOutcome_Requested value)? requested,
    TResult Function(VideoStartOutcome_Unavailable value)? unavailable,
    TResult Function(VideoStartOutcome_NoSession value)? noSession,
    TResult Function(VideoStartOutcome_AlreadyActive value)? alreadyActive,
    TResult Function(VideoStartOutcome_Failed value)? failed,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested() when requested != null:
        return requested(_that);
      case VideoStartOutcome_Unavailable() when unavailable != null:
        return unavailable(_that);
      case VideoStartOutcome_NoSession() when noSession != null:
        return noSession(_that);
      case VideoStartOutcome_AlreadyActive() when alreadyActive != null:
        return alreadyActive(_that);
      case VideoStartOutcome_Failed() when failed != null:
        return failed(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(VideoStartOutcome_Requested value) requested,
    required TResult Function(VideoStartOutcome_Unavailable value) unavailable,
    required TResult Function(VideoStartOutcome_NoSession value) noSession,
    required TResult Function(VideoStartOutcome_AlreadyActive value)
        alreadyActive,
    required TResult Function(VideoStartOutcome_Failed value) failed,
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested():
        return requested(_that);
      case VideoStartOutcome_Unavailable():
        return unavailable(_that);
      case VideoStartOutcome_NoSession():
        return noSession(_that);
      case VideoStartOutcome_AlreadyActive():
        return alreadyActive(_that);
      case VideoStartOutcome_Failed():
        return failed(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(VideoStartOutcome_Requested value)? requested,
    TResult? Function(VideoStartOutcome_Unavailable value)? unavailable,
    TResult? Function(VideoStartOutcome_NoSession value)? noSession,
    TResult? Function(VideoStartOutcome_AlreadyActive value)? alreadyActive,
    TResult? Function(VideoStartOutcome_Failed value)? failed,
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested() when requested != null:
        return requested(_that);
      case VideoStartOutcome_Unavailable() when unavailable != null:
        return unavailable(_that);
      case VideoStartOutcome_NoSession() when noSession != null:
        return noSession(_that);
      case VideoStartOutcome_AlreadyActive() when alreadyActive != null:
        return alreadyActive(_that);
      case VideoStartOutcome_Failed() when failed != null:
        return failed(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function(VideoSessionIdentity field0)? requested,
    TResult Function(VideoUnavailable field0)? unavailable,
    TResult Function()? noSession,
    TResult Function()? alreadyActive,
    TResult Function(VideoTerminalReason field0)? failed,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested() when requested != null:
        return requested(_that.field0);
      case VideoStartOutcome_Unavailable() when unavailable != null:
        return unavailable(_that.field0);
      case VideoStartOutcome_NoSession() when noSession != null:
        return noSession();
      case VideoStartOutcome_AlreadyActive() when alreadyActive != null:
        return alreadyActive();
      case VideoStartOutcome_Failed() when failed != null:
        return failed(_that.field0);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function(VideoSessionIdentity field0) requested,
    required TResult Function(VideoUnavailable field0) unavailable,
    required TResult Function() noSession,
    required TResult Function() alreadyActive,
    required TResult Function(VideoTerminalReason field0) failed,
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested():
        return requested(_that.field0);
      case VideoStartOutcome_Unavailable():
        return unavailable(_that.field0);
      case VideoStartOutcome_NoSession():
        return noSession();
      case VideoStartOutcome_AlreadyActive():
        return alreadyActive();
      case VideoStartOutcome_Failed():
        return failed(_that.field0);
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function(VideoSessionIdentity field0)? requested,
    TResult? Function(VideoUnavailable field0)? unavailable,
    TResult? Function()? noSession,
    TResult? Function()? alreadyActive,
    TResult? Function(VideoTerminalReason field0)? failed,
  }) {
    final _that = this;
    switch (_that) {
      case VideoStartOutcome_Requested() when requested != null:
        return requested(_that.field0);
      case VideoStartOutcome_Unavailable() when unavailable != null:
        return unavailable(_that.field0);
      case VideoStartOutcome_NoSession() when noSession != null:
        return noSession();
      case VideoStartOutcome_AlreadyActive() when alreadyActive != null:
        return alreadyActive();
      case VideoStartOutcome_Failed() when failed != null:
        return failed(_that.field0);
      case _:
        return null;
    }
  }
}

/// @nodoc

class VideoStartOutcome_Requested extends VideoStartOutcome {
  const VideoStartOutcome_Requested(this.field0) : super._();

  final VideoSessionIdentity field0;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoStartOutcome_RequestedCopyWith<VideoStartOutcome_Requested>
      get copyWith => _$VideoStartOutcome_RequestedCopyWithImpl<
          VideoStartOutcome_Requested>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoStartOutcome_Requested &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoStartOutcome.requested(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoStartOutcome_RequestedCopyWith<$Res>
    implements $VideoStartOutcomeCopyWith<$Res> {
  factory $VideoStartOutcome_RequestedCopyWith(
          VideoStartOutcome_Requested value,
          $Res Function(VideoStartOutcome_Requested) _then) =
      _$VideoStartOutcome_RequestedCopyWithImpl;
  @useResult
  $Res call({VideoSessionIdentity field0});
}

/// @nodoc
class _$VideoStartOutcome_RequestedCopyWithImpl<$Res>
    implements $VideoStartOutcome_RequestedCopyWith<$Res> {
  _$VideoStartOutcome_RequestedCopyWithImpl(this._self, this._then);

  final VideoStartOutcome_Requested _self;
  final $Res Function(VideoStartOutcome_Requested) _then;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoStartOutcome_Requested(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoSessionIdentity,
    ));
  }
}

/// @nodoc

class VideoStartOutcome_Unavailable extends VideoStartOutcome {
  const VideoStartOutcome_Unavailable(this.field0) : super._();

  final VideoUnavailable field0;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoStartOutcome_UnavailableCopyWith<VideoStartOutcome_Unavailable>
      get copyWith => _$VideoStartOutcome_UnavailableCopyWithImpl<
          VideoStartOutcome_Unavailable>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoStartOutcome_Unavailable &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoStartOutcome.unavailable(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoStartOutcome_UnavailableCopyWith<$Res>
    implements $VideoStartOutcomeCopyWith<$Res> {
  factory $VideoStartOutcome_UnavailableCopyWith(
          VideoStartOutcome_Unavailable value,
          $Res Function(VideoStartOutcome_Unavailable) _then) =
      _$VideoStartOutcome_UnavailableCopyWithImpl;
  @useResult
  $Res call({VideoUnavailable field0});

  $VideoUnavailableCopyWith<$Res> get field0;
}

/// @nodoc
class _$VideoStartOutcome_UnavailableCopyWithImpl<$Res>
    implements $VideoStartOutcome_UnavailableCopyWith<$Res> {
  _$VideoStartOutcome_UnavailableCopyWithImpl(this._self, this._then);

  final VideoStartOutcome_Unavailable _self;
  final $Res Function(VideoStartOutcome_Unavailable) _then;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoStartOutcome_Unavailable(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoUnavailable,
    ));
  }

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @override
  @pragma('vm:prefer-inline')
  $VideoUnavailableCopyWith<$Res> get field0 {
    return $VideoUnavailableCopyWith<$Res>(_self.field0, (value) {
      return _then(_self.copyWith(field0: value));
    });
  }
}

/// @nodoc

class VideoStartOutcome_NoSession extends VideoStartOutcome {
  const VideoStartOutcome_NoSession() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoStartOutcome_NoSession);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoStartOutcome.noSession()';
  }
}

/// @nodoc

class VideoStartOutcome_AlreadyActive extends VideoStartOutcome {
  const VideoStartOutcome_AlreadyActive() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoStartOutcome_AlreadyActive);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoStartOutcome.alreadyActive()';
  }
}

/// @nodoc

class VideoStartOutcome_Failed extends VideoStartOutcome {
  const VideoStartOutcome_Failed(this.field0) : super._();

  final VideoTerminalReason field0;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoStartOutcome_FailedCopyWith<VideoStartOutcome_Failed> get copyWith =>
      _$VideoStartOutcome_FailedCopyWithImpl<VideoStartOutcome_Failed>(
          this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoStartOutcome_Failed &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoStartOutcome.failed(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoStartOutcome_FailedCopyWith<$Res>
    implements $VideoStartOutcomeCopyWith<$Res> {
  factory $VideoStartOutcome_FailedCopyWith(VideoStartOutcome_Failed value,
          $Res Function(VideoStartOutcome_Failed) _then) =
      _$VideoStartOutcome_FailedCopyWithImpl;
  @useResult
  $Res call({VideoTerminalReason field0});
}

/// @nodoc
class _$VideoStartOutcome_FailedCopyWithImpl<$Res>
    implements $VideoStartOutcome_FailedCopyWith<$Res> {
  _$VideoStartOutcome_FailedCopyWithImpl(this._self, this._then);

  final VideoStartOutcome_Failed _self;
  final $Res Function(VideoStartOutcome_Failed) _then;

  /// Create a copy of VideoStartOutcome
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoStartOutcome_Failed(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoTerminalReason,
    ));
  }
}

/// @nodoc
mixin _$VideoUnavailable {
  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType && other is VideoUnavailable);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoUnavailable()';
  }
}

/// @nodoc
class $VideoUnavailableCopyWith<$Res> {
  $VideoUnavailableCopyWith(
      VideoUnavailable _, $Res Function(VideoUnavailable) __);
}

/// Adds pattern-matching-related methods to [VideoUnavailable].
extension VideoUnavailablePatterns on VideoUnavailable {
  /// A variant of `map` that fallback to returning `orElse`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeMap<TResult extends Object?>({
    TResult Function(VideoUnavailable_PlatformUnsupported value)?
        platformUnsupported,
    TResult Function(VideoUnavailable_RuntimeUnavailable value)?
        runtimeUnavailable,
    TResult Function(VideoUnavailable_SourceUnavailable value)?
        sourceUnavailable,
    TResult Function(VideoUnavailable_FormatUnavailable value)?
        formatUnavailable,
    TResult Function(VideoUnavailable_ConfigurationUnavailable value)?
        configurationUnavailable,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported()
          when platformUnsupported != null:
        return platformUnsupported(_that);
      case VideoUnavailable_RuntimeUnavailable()
          when runtimeUnavailable != null:
        return runtimeUnavailable(_that);
      case VideoUnavailable_SourceUnavailable() when sourceUnavailable != null:
        return sourceUnavailable(_that);
      case VideoUnavailable_FormatUnavailable() when formatUnavailable != null:
        return formatUnavailable(_that);
      case VideoUnavailable_ConfigurationUnavailable()
          when configurationUnavailable != null:
        return configurationUnavailable(_that);
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// Callbacks receives the raw object, upcasted.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case final Subclass2 value:
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult map<TResult extends Object?>({
    required TResult Function(VideoUnavailable_PlatformUnsupported value)
        platformUnsupported,
    required TResult Function(VideoUnavailable_RuntimeUnavailable value)
        runtimeUnavailable,
    required TResult Function(VideoUnavailable_SourceUnavailable value)
        sourceUnavailable,
    required TResult Function(VideoUnavailable_FormatUnavailable value)
        formatUnavailable,
    required TResult Function(VideoUnavailable_ConfigurationUnavailable value)
        configurationUnavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported():
        return platformUnsupported(_that);
      case VideoUnavailable_RuntimeUnavailable():
        return runtimeUnavailable(_that);
      case VideoUnavailable_SourceUnavailable():
        return sourceUnavailable(_that);
      case VideoUnavailable_FormatUnavailable():
        return formatUnavailable(_that);
      case VideoUnavailable_ConfigurationUnavailable():
        return configurationUnavailable(_that);
    }
  }

  /// A variant of `map` that fallback to returning `null`.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case final Subclass value:
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? mapOrNull<TResult extends Object?>({
    TResult? Function(VideoUnavailable_PlatformUnsupported value)?
        platformUnsupported,
    TResult? Function(VideoUnavailable_RuntimeUnavailable value)?
        runtimeUnavailable,
    TResult? Function(VideoUnavailable_SourceUnavailable value)?
        sourceUnavailable,
    TResult? Function(VideoUnavailable_FormatUnavailable value)?
        formatUnavailable,
    TResult? Function(VideoUnavailable_ConfigurationUnavailable value)?
        configurationUnavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported()
          when platformUnsupported != null:
        return platformUnsupported(_that);
      case VideoUnavailable_RuntimeUnavailable()
          when runtimeUnavailable != null:
        return runtimeUnavailable(_that);
      case VideoUnavailable_SourceUnavailable() when sourceUnavailable != null:
        return sourceUnavailable(_that);
      case VideoUnavailable_FormatUnavailable() when formatUnavailable != null:
        return formatUnavailable(_that);
      case VideoUnavailable_ConfigurationUnavailable()
          when configurationUnavailable != null:
        return configurationUnavailable(_that);
      case _:
        return null;
    }
  }

  /// A variant of `when` that fallback to an `orElse` callback.
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return orElse();
  /// }
  /// ```

  @optionalTypeArgs
  TResult maybeWhen<TResult extends Object?>({
    TResult Function()? platformUnsupported,
    TResult Function()? runtimeUnavailable,
    TResult Function(VideoSource field0)? sourceUnavailable,
    TResult Function(VideoMediaFormat field0)? formatUnavailable,
    TResult Function()? configurationUnavailable,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported()
          when platformUnsupported != null:
        return platformUnsupported();
      case VideoUnavailable_RuntimeUnavailable()
          when runtimeUnavailable != null:
        return runtimeUnavailable();
      case VideoUnavailable_SourceUnavailable() when sourceUnavailable != null:
        return sourceUnavailable(_that.field0);
      case VideoUnavailable_FormatUnavailable() when formatUnavailable != null:
        return formatUnavailable(_that.field0);
      case VideoUnavailable_ConfigurationUnavailable()
          when configurationUnavailable != null:
        return configurationUnavailable();
      case _:
        return orElse();
    }
  }

  /// A `switch`-like method, using callbacks.
  ///
  /// As opposed to `map`, this offers destructuring.
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case Subclass2(:final field2):
  ///     return ...;
  /// }
  /// ```

  @optionalTypeArgs
  TResult when<TResult extends Object?>({
    required TResult Function() platformUnsupported,
    required TResult Function() runtimeUnavailable,
    required TResult Function(VideoSource field0) sourceUnavailable,
    required TResult Function(VideoMediaFormat field0) formatUnavailable,
    required TResult Function() configurationUnavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported():
        return platformUnsupported();
      case VideoUnavailable_RuntimeUnavailable():
        return runtimeUnavailable();
      case VideoUnavailable_SourceUnavailable():
        return sourceUnavailable(_that.field0);
      case VideoUnavailable_FormatUnavailable():
        return formatUnavailable(_that.field0);
      case VideoUnavailable_ConfigurationUnavailable():
        return configurationUnavailable();
    }
  }

  /// A variant of `when` that fallback to returning `null`
  ///
  /// It is equivalent to doing:
  /// ```dart
  /// switch (sealedClass) {
  ///   case Subclass(:final field):
  ///     return ...;
  ///   case _:
  ///     return null;
  /// }
  /// ```

  @optionalTypeArgs
  TResult? whenOrNull<TResult extends Object?>({
    TResult? Function()? platformUnsupported,
    TResult? Function()? runtimeUnavailable,
    TResult? Function(VideoSource field0)? sourceUnavailable,
    TResult? Function(VideoMediaFormat field0)? formatUnavailable,
    TResult? Function()? configurationUnavailable,
  }) {
    final _that = this;
    switch (_that) {
      case VideoUnavailable_PlatformUnsupported()
          when platformUnsupported != null:
        return platformUnsupported();
      case VideoUnavailable_RuntimeUnavailable()
          when runtimeUnavailable != null:
        return runtimeUnavailable();
      case VideoUnavailable_SourceUnavailable() when sourceUnavailable != null:
        return sourceUnavailable(_that.field0);
      case VideoUnavailable_FormatUnavailable() when formatUnavailable != null:
        return formatUnavailable(_that.field0);
      case VideoUnavailable_ConfigurationUnavailable()
          when configurationUnavailable != null:
        return configurationUnavailable();
      case _:
        return null;
    }
  }
}

/// @nodoc

class VideoUnavailable_PlatformUnsupported extends VideoUnavailable {
  const VideoUnavailable_PlatformUnsupported() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoUnavailable_PlatformUnsupported);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoUnavailable.platformUnsupported()';
  }
}

/// @nodoc

class VideoUnavailable_RuntimeUnavailable extends VideoUnavailable {
  const VideoUnavailable_RuntimeUnavailable() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoUnavailable_RuntimeUnavailable);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoUnavailable.runtimeUnavailable()';
  }
}

/// @nodoc

class VideoUnavailable_SourceUnavailable extends VideoUnavailable {
  const VideoUnavailable_SourceUnavailable(this.field0) : super._();

  final VideoSource field0;

  /// Create a copy of VideoUnavailable
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoUnavailable_SourceUnavailableCopyWith<
          VideoUnavailable_SourceUnavailable>
      get copyWith => _$VideoUnavailable_SourceUnavailableCopyWithImpl<
          VideoUnavailable_SourceUnavailable>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoUnavailable_SourceUnavailable &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoUnavailable.sourceUnavailable(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoUnavailable_SourceUnavailableCopyWith<$Res>
    implements $VideoUnavailableCopyWith<$Res> {
  factory $VideoUnavailable_SourceUnavailableCopyWith(
          VideoUnavailable_SourceUnavailable value,
          $Res Function(VideoUnavailable_SourceUnavailable) _then) =
      _$VideoUnavailable_SourceUnavailableCopyWithImpl;
  @useResult
  $Res call({VideoSource field0});
}

/// @nodoc
class _$VideoUnavailable_SourceUnavailableCopyWithImpl<$Res>
    implements $VideoUnavailable_SourceUnavailableCopyWith<$Res> {
  _$VideoUnavailable_SourceUnavailableCopyWithImpl(this._self, this._then);

  final VideoUnavailable_SourceUnavailable _self;
  final $Res Function(VideoUnavailable_SourceUnavailable) _then;

  /// Create a copy of VideoUnavailable
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoUnavailable_SourceUnavailable(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoSource,
    ));
  }
}

/// @nodoc

class VideoUnavailable_FormatUnavailable extends VideoUnavailable {
  const VideoUnavailable_FormatUnavailable(this.field0) : super._();

  final VideoMediaFormat field0;

  /// Create a copy of VideoUnavailable
  /// with the given fields replaced by the non-null parameter values.
  @JsonKey(includeFromJson: false, includeToJson: false)
  @pragma('vm:prefer-inline')
  $VideoUnavailable_FormatUnavailableCopyWith<
          VideoUnavailable_FormatUnavailable>
      get copyWith => _$VideoUnavailable_FormatUnavailableCopyWithImpl<
          VideoUnavailable_FormatUnavailable>(this, _$identity);

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoUnavailable_FormatUnavailable &&
            (identical(other.field0, field0) || other.field0 == field0));
  }

  @override
  int get hashCode => Object.hash(runtimeType, field0);

  @override
  String toString() {
    return 'VideoUnavailable.formatUnavailable(field0: $field0)';
  }
}

/// @nodoc
abstract mixin class $VideoUnavailable_FormatUnavailableCopyWith<$Res>
    implements $VideoUnavailableCopyWith<$Res> {
  factory $VideoUnavailable_FormatUnavailableCopyWith(
          VideoUnavailable_FormatUnavailable value,
          $Res Function(VideoUnavailable_FormatUnavailable) _then) =
      _$VideoUnavailable_FormatUnavailableCopyWithImpl;
  @useResult
  $Res call({VideoMediaFormat field0});

  $VideoMediaFormatCopyWith<$Res> get field0;
}

/// @nodoc
class _$VideoUnavailable_FormatUnavailableCopyWithImpl<$Res>
    implements $VideoUnavailable_FormatUnavailableCopyWith<$Res> {
  _$VideoUnavailable_FormatUnavailableCopyWithImpl(this._self, this._then);

  final VideoUnavailable_FormatUnavailable _self;
  final $Res Function(VideoUnavailable_FormatUnavailable) _then;

  /// Create a copy of VideoUnavailable
  /// with the given fields replaced by the non-null parameter values.
  @pragma('vm:prefer-inline')
  $Res call({
    Object? field0 = null,
  }) {
    return _then(VideoUnavailable_FormatUnavailable(
      null == field0
          ? _self.field0
          : field0 // ignore: cast_nullable_to_non_nullable
              as VideoMediaFormat,
    ));
  }

  /// Create a copy of VideoUnavailable
  /// with the given fields replaced by the non-null parameter values.
  @override
  @pragma('vm:prefer-inline')
  $VideoMediaFormatCopyWith<$Res> get field0 {
    return $VideoMediaFormatCopyWith<$Res>(_self.field0, (value) {
      return _then(_self.copyWith(field0: value));
    });
  }
}

/// @nodoc

class VideoUnavailable_ConfigurationUnavailable extends VideoUnavailable {
  const VideoUnavailable_ConfigurationUnavailable() : super._();

  @override
  bool operator ==(Object other) {
    return identical(this, other) ||
        (other.runtimeType == runtimeType &&
            other is VideoUnavailable_ConfigurationUnavailable);
  }

  @override
  int get hashCode => runtimeType.hashCode;

  @override
  String toString() {
    return 'VideoUnavailable.configurationUnavailable()';
  }
}

// dart format on
