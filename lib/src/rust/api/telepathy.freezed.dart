// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'telepathy.dart';

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

// dart format on
