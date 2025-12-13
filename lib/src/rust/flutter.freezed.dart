// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'flutter.dart';

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
    TResult Function(bool relayed)? connected,
    TResult Function()? inactive,
    TResult Function()? unknown,
    required TResult orElse(),
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting();
      case SessionStatus_Connected() when connected != null:
        return connected(_that.relayed);
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
    required TResult Function(bool relayed) connected,
    required TResult Function() inactive,
    required TResult Function() unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting():
        return connecting();
      case SessionStatus_Connected():
        return connected(_that.relayed);
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
    TResult? Function(bool relayed)? connected,
    TResult? Function()? inactive,
    TResult? Function()? unknown,
  }) {
    final _that = this;
    switch (_that) {
      case SessionStatus_Connecting() when connecting != null:
        return connecting();
      case SessionStatus_Connected() when connected != null:
        return connected(_that.relayed);
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
  const SessionStatus_Connected({required this.relayed}) : super._();

  final bool relayed;

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
            (identical(other.relayed, relayed) || other.relayed == relayed));
  }

  @override
  int get hashCode => Object.hash(runtimeType, relayed);

  @override
  String toString() {
    return 'SessionStatus.connected(relayed: $relayed)';
  }
}

/// @nodoc
abstract mixin class $SessionStatus_ConnectedCopyWith<$Res>
    implements $SessionStatusCopyWith<$Res> {
  factory $SessionStatus_ConnectedCopyWith(SessionStatus_Connected value,
          $Res Function(SessionStatus_Connected) _then) =
      _$SessionStatus_ConnectedCopyWithImpl;
  @useResult
  $Res call({bool relayed});
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
  }) {
    return _then(SessionStatus_Connected(
      relayed: null == relayed
          ? _self.relayed
          : relayed // ignore: cast_nullable_to_non_nullable
              as bool,
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

// dart format on
