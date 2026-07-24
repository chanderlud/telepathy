import 'dart:async';
import 'dart:core';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
import 'package:telepathy/core/utils/console.dart';
import 'package:telepathy/models/index.dart';
import 'package:telepathy/widgets/common/index.dart';

class ProfileSettings extends StatefulWidget {
  const ProfileSettings({super.key});

  @override
  ProfileSettingsState createState() => ProfileSettingsState();
}

class ProfileSettingsState extends State<ProfileSettings> {
  final TextEditingController _profileNameInput = TextEditingController();
  String? _profileNameError;

  /// Latest delete-attempt failure keyed by profile id, surfaced in the
  /// confirmation dialog so the user can retry. Previously the dialog
  /// closed silently on failure, hiding storage-cleanup errors that left
  /// private keys on disk.
  ///
  /// The phase drives the user-facing message so each failure mode gets
  /// an accurate description instead of always claiming the index update
  /// succeeded.
  ProfileDeletionException? _deleteProfileError;
  String? _deleteProfileErrorId;

  /// True while a manual cleanup retry is in flight so the dialog can
  /// disable its retry button.
  bool _cleanupRetryInFlight = false;

  String _deleteErrorMessage(BuildContext context) {
    final failure = _deleteProfileError;
    if (failure == null) {
      return '';
    }
    final cause = '${failure.cause}';
    switch (failure.phase) {
      case ProfileDeletionPhase.tombstoneWrite:
        return 'Could not record the deletion intent. No data was changed. '
            'You can retry the delete or close this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.begin:
        return 'The backend rejected the identity switch. No data was '
            'changed. You can retry or close this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.activeIdPersist:
        return 'Could not persist the replacement active profile. The '
            'transaction was cancelled; no data was changed. You can '
            'retry or close this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.indexWrite:
        return 'Could not update the profile index. The transaction was '
            'rolled back; no data was changed. You can retry or close '
            'this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.commit:
        return 'The backend could not commit the new identity. The '
            'transaction was rolled back across the frontend, preferences, '
            'and backend; no data was changed. You can retry or close '
            'this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.replacementCreate:
        return 'Could not create a replacement profile for the deletion. '
            'The active profile is unchanged; no data was modified. You '
            'can retry or close this dialog.\nDetails: $cause';
      case ProfileDeletionPhase.storageCleanup:
        return 'The profile was removed from the index but its private-key '
            'records could not be deleted. Cleanup will retry '
            'automatically on the next startup. You can retry now or '
            'close this dialog.\nDetails: $cause';
    }
  }

  InputDecoration _profileNameInputDecoration(BuildContext context) {
    if (_profileNameError == null) {
      return const InputDecoration(labelText: 'Name');
    }

    final errorColor = Theme.of(context).colorScheme.error;
    final errorHoverColor = Color.lerp(errorColor, Colors.black, 0.16)!;

    return InputDecoration(
      labelText: 'Name',
      errorText: _profileNameError,
      labelStyle: WidgetStateTextStyle.resolveWith((Set<WidgetState> states) {
        return TextStyle(
          color: states.contains(WidgetState.hovered)
              ? errorHoverColor
              : errorColor,
        );
      }),
      floatingLabelStyle:
          WidgetStateTextStyle.resolveWith((Set<WidgetState> states) {
        return TextStyle(
          color: states.contains(WidgetState.hovered)
              ? errorHoverColor
              : errorColor,
        );
      }),
      border: WidgetStateInputBorder.resolveWith((Set<WidgetState> states) {
        final hovered = states.contains(WidgetState.hovered);
        final focused = states.contains(WidgetState.focused);

        return UnderlineInputBorder(
          borderSide: BorderSide(
            color: hovered ? errorHoverColor : errorColor,
            width: focused ? 2 : 1,
          ),
        );
      }),
    );
  }

  void _createProfile(
    BuildContext dialogContext,
    ProfilesController profilesController,
    StateSetter setDialogState,
  ) {
    final profileName = _profileNameInput.text.trim();

    if (profileName.isEmpty) {
      setDialogState(() {
        _profileNameError = 'Profile name is required.';
      });
      return;
    }

    final profileNameExists = profilesController.profiles.values.any(
      (Profile profile) =>
          profile.nickname.trim().toLowerCase() == profileName.toLowerCase(),
    );

    if (profileNameExists) {
      setDialogState(() {
        _profileNameError = 'A profile named "$profileName" already exists.';
      });
      return;
    }

    profilesController.createProfile(profileName);
    _profileNameInput.clear();
    _profileNameError = null;
    Navigator.of(dialogContext).pop();
  }

  @override
  Widget build(BuildContext context) {
    final profilesController = context.watch<ProfilesController>();
    final stateController = context.watch<StateController>();
    final telepathy = context.read<Telepathy>();

    return Container(
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.surface,
        borderRadius: BorderRadius.circular(5),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.end,
        mainAxisSize: MainAxisSize.min,
        children: [
          Builder(builder: (BuildContext builderContext) {
            bool even = profilesController.profiles.length % 2 == 0;

            Color colorPicker(int index) {
              if (even ? index % 2 == 0 : index % 2 != 0) {
                return Colors.transparent;
              } else {
                return Theme.of(builderContext).colorScheme.secondaryContainer;
              }
            }

            return ListView.builder(
                shrinkWrap: true,
                itemCount: profilesController.profiles.length,
                itemBuilder: (BuildContext listContext, int index) {
                  Profile profile =
                      profilesController.profiles.values.elementAt(index);

                  return Container(
                    decoration: BoxDecoration(
                      color: colorPicker(index),
                      borderRadius: index == 0
                          ? const BorderRadius.only(
                              topLeft: Radius.circular(5),
                              topRight: Radius.circular(5))
                          : null,
                    ),
                    padding: const EdgeInsets.only(
                        top: 5, bottom: 5, left: 20, right: 10),
                    child: Row(
                      children: [
                        Text(
                          profile.nickname,
                          style: const TextStyle(fontSize: 18),
                        ),
                        const Spacer(),
                        Button(
                          text: (profilesController.activeProfile == profile.id)
                              ? 'Active'
                              : 'Set Active',
                          width: 65,
                          height: 25,
                          // `blockAudioChanges` covers the connecting and
                          // active phases and audio tests, all of which
                          // occupy the backend call slot and would race
                          // with the atomic identity swap. Using
                          // `isCallActive` here is not enough because the
                          // slot is occupied before promotion.
                          //
                          // `isIdentitySwitchPending` covers the in-flight
                          // two-phase transaction itself so a second switch
                          // cannot begin while the first is still
                          // committing/cancelling across both layers.
                          disabled: stateController.blockAudioChanges ||
                              profilesController.isIdentitySwitchPending ||
                              profilesController.activeProfile == profile.id,
                          onPressed: () async {
                            // Defensive recheck inside the handler so a
                            // build-cycle race between `disabled` being
                            // painted and the user tapping cannot reach the
                            // mutating two-phase transaction.
                            if (stateController.blockAudioChanges ||
                                profilesController.isIdentitySwitchPending) {
                              return;
                            }
                            // The controller orchestrates the two-phase
                            // transaction: acquire the backend gate,
                            // persist the target active profile, then
                            // commit the new signing key + contact
                            // snapshot. On any failure it rolls the
                            // frontend back to the previous active profile
                            // and either cancels (pre-commit) or relies on
                            // Rust's internal rollback (post-commit). The
                            // error is logged but not rethrown: this
                            // handler runs inside a tap callback, and
                            // propagating would surface as an unhandled
                            // exception with no UI to recover it.
                            try {
                              await profilesController.switchActiveProfile(
                                profile.id,
                                telepathy: telepathy,
                              );
                            } catch (error) {
                              DebugConsole.warn(
                                'switchActiveProfile failed for '
                                '${profile.id}: $error; '
                                'frontend active profile left unchanged',
                              );
                            }
                          },
                          noSplash: true,
                          disabledColor:
                              profilesController.activeProfile == profile.id &&
                                      (stateController.blockAudioChanges ||
                                          profilesController
                                              .isIdentitySwitchPending)
                                  ? Theme.of(listContext)
                                      .colorScheme
                                      .tertiaryContainer
                                  : null,
                        ),
                        const SizedBox(width: 10),
                        IconButton(
                            tooltip: 'Copy Peer ID',
                            onPressed: () {
                              Clipboard.setData(
                                  ClipboardData(text: profile.peerId));
                            },
                            icon: SvgPicture.asset(
                              'assets/icons/Copy.svg',
                              semanticsLabel: 'Copy Peer ID',
                              width: 26,
                            )),
                        IconButton(
                          tooltip: 'Delete Profile',
                          // Deleting the active profile must run the same
                          // two-phase identity-switch transaction the Set
                          // Active button uses; both must be gated by
                          // `blockAudioChanges` so the backend identity
                          // switch + manager restart cannot race an
                          // in-flight call. Deleting a non-active profile
                          // is safe during a call because it touches
                          // neither the call slot nor the active identity.
                          //
                          // `isIdentitySwitchPending` additionally blocks
                          // every delete while another transaction is in
                          // flight, including non-active deletes, so the
                          // transaction's frontend persistence cannot be
                          // raced by a sibling deletion.
                          onPressed: (stateController.blockAudioChanges &&
                                      profilesController.activeProfile ==
                                          profile.id) ||
                                  profilesController.isIdentitySwitchPending
                              ? null
                              : () {
                                  setState(() {
                                    _deleteProfileError = null;
                                    _deleteProfileErrorId = null;
                                    _cleanupRetryInFlight = false;
                                  });
                                  showDialog(
                                      context: listContext,
                                      builder: (BuildContext dialogContext) {
                                        return StatefulBuilder(builder:
                                            (BuildContext dialogContext,
                                                StateSetter setDialogState) {
                                          return AlertDialog(
                                            title: const Text('Delete Profile'),
                                            content: Column(
                                              mainAxisSize: MainAxisSize.min,
                                              crossAxisAlignment:
                                                  CrossAxisAlignment.start,
                                              children: [
                                                const Text(
                                                    'Are you sure you want to delete this profile?'),
                                                if (_deleteProfileError != null)
                                                  Padding(
                                                    padding:
                                                        const EdgeInsets.only(
                                                            top: 12),
                                                    child: Text(
                                                      _deleteErrorMessage(
                                                          dialogContext),
                                                      style: TextStyle(
                                                        color: Theme.of(
                                                                dialogContext)
                                                            .colorScheme
                                                            .error,
                                                      ),
                                                    ),
                                                  ),
                                              ],
                                            ),
                                            actions: [
                                              Button(
                                                text: 'Cancel',
                                                onPressed: () {
                                                  Navigator.of(dialogContext)
                                                      .pop();
                                                },
                                              ),
                                              // "Retry Cleanup" appears
                                              // ONLY when the controller
                                              // surfaced a failure that
                                              // left a durable tombstone
                                              // (i.e.
                                              // `tombstonedForStartupRetry`
                                              // is true). Routing on the
                                              // phase alone was unsafe: a
                                              // generic catch-all used to
                                              // mis-classify ANY non-typed
                                              // error as `storageCleanup`,
                                              // exposing the destructive
                                              // cleanup retry on failures
                                              // that never left a
                                              // tombstone (and could be
                                              // for the still-live active
                                              // profile).
                                              if (_deleteProfileError != null &&
                                                  _deleteProfileError!
                                                      .tombstonedForStartupRetry &&
                                                  _deleteProfileErrorId != null)
                                                Button(
                                                  text: _cleanupRetryInFlight
                                                      ? 'Retrying...'
                                                      : 'Retry Cleanup',
                                                  disabled:
                                                      _cleanupRetryInFlight,
                                                  onPressed: () {
                                                    final id =
                                                        _deleteProfileErrorId!;
                                                    final navigator =
                                                        Navigator.of(
                                                            dialogContext);
                                                    setDialogState(() {
                                                      _cleanupRetryInFlight =
                                                          true;
                                                    });
                                                    unawaited(profilesController
                                                        .retryDeletionCleanup(
                                                            id)
                                                        .then((ok) {
                                                      if (!mounted) {
                                                        return;
                                                      }
                                                      if (ok) {
                                                        navigator.pop();
                                                      } else {
                                                        setDialogState(() {
                                                          _cleanupRetryInFlight =
                                                              false;
                                                        });
                                                      }
                                                    }));
                                                  },
                                                ),
                                              // The ordinary
                                              // "Delete"/"Retry" button
                                              // drives `removeProfile`. It
                                              // is hidden once the index
                                              // deletion completed
                                              // (`tombstonedForStartupRetry`
                                              // is true): at that point
                                              // the id is already gone
                                              // from the index, so
                                              // re-issuing `removeProfile`
                                              // would be a no-op
                                              // (`!profiles.containsKey(id)`
                                              // early-returns) and
                                              // mislead the user. Only
                                              // the dedicated "Retry
                                              // Cleanup" above is offered
                                              // in that state.
                                              if (_deleteProfileError == null ||
                                                  !_deleteProfileError!
                                                      .tombstonedForStartupRetry)
                                                Button(
                                                  text: _deleteProfileError ==
                                                          null
                                                      ? 'Delete'
                                                      : 'Retry',
                                                  onPressed: () async {
                                                    final navigator =
                                                        Navigator.of(
                                                            dialogContext);
                                                    try {
                                                      await profilesController
                                                          .removeProfile(
                                                        profile.id,
                                                        telepathy: telepathy,
                                                      );
                                                      if (mounted) {
                                                        navigator.pop();
                                                      }
                                                    } on ProfileDeletionException catch (error) {
                                                      DebugConsole.warn(
                                                        'delete of profile '
                                                        '${profile.id} failed '
                                                        '(${error.phase}): '
                                                        '${error.cause}',
                                                      );
                                                      setDialogState(() {
                                                        _deleteProfileError =
                                                            error;
                                                        _deleteProfileErrorId =
                                                            profile.id;
                                                        _cleanupRetryInFlight =
                                                            false;
                                                      });
                                                    }
                                                  },
                                                )
                                            ],
                                          );
                                        });
                                      });
                                },
                          icon: SvgPicture.asset(
                            'assets/icons/Trash.svg',
                            semanticsLabel: 'Delete Profile',
                            width: 26,
                          ),
                        ),
                      ],
                    ),
                  );
                });
          }),
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 5, horizontal: 20),
            child: IconButton(
              onPressed: profilesController.isIdentitySwitchPending
                  ? null
                  : () {
                      _profileNameError = null;
                      showDialog(
                          context: context,
                          builder: (BuildContext context) {
                            return StatefulBuilder(
                              builder: (BuildContext context,
                                  StateSetter setDialogState) {
                                return CallbackShortcuts(
                                  bindings: <ShortcutActivator, VoidCallback>{
                                    const SingleActivator(
                                            LogicalKeyboardKey.enter):
                                        () => _createProfile(context,
                                            profilesController, setDialogState),
                                  },
                                  child: SimpleDialog(
                                    title: const Text('Create Profile'),
                                    contentPadding: const EdgeInsets.only(
                                        bottom: 25, left: 25, right: 25),
                                    titlePadding: const EdgeInsets.only(
                                        top: 25,
                                        left: 25,
                                        right: 25,
                                        bottom: 15),
                                    children: [
                                      TextField(
                                        decoration: _profileNameInputDecoration(
                                            context),
                                        controller: _profileNameInput,
                                        onChanged: (_) {
                                          if (_profileNameError == null) return;

                                          setDialogState(() {
                                            _profileNameError = null;
                                          });
                                        },
                                        onSubmitted: (_) => _createProfile(
                                            context,
                                            profilesController,
                                            setDialogState),
                                      ),
                                      const SizedBox(height: 20),
                                      Button(
                                        text: 'Create',
                                        onPressed: () => _createProfile(context,
                                            profilesController, setDialogState),
                                      )
                                    ],
                                  ),
                                );
                              },
                            );
                          });
                    },
              visualDensity: VisualDensity.comfortable,
              icon: SvgPicture.asset(
                'assets/icons/Plus.svg',
                semanticsLabel: 'Create Profile',
                width: 38,
              ),
              tooltip: 'Create Profile',
            ),
          ),
        ],
      ),
    );
  }
}
