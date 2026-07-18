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
                          disabled: stateController.blockAudioChanges ||
                              profilesController.activeProfile == profile.id,
                          onPressed: () async {
                            // Defensive recheck inside the handler so a
                            // build-cycle race between `disabled` being
                            // painted and the user tapping cannot reach the
                            // mutating atomic identity swap.
                            if (stateController.blockAudioChanges) {
                              return;
                            }
                            // Commit the frontend active-profile change only
                            // after the atomic backend op succeeds. If the
                            // backend rejects the swap the frontend stays on
                            // the current profile. The error is logged but
                            // not rethrown: this handler runs inside a tap
                            // callback, and propagating would surface as an
                            // unhandled exception with no UI to recover it.
                            try {
                              await telepathy.switchIdentityAndRestartManager(
                                key: profile.keypair,
                              );
                              await profilesController
                                  .setActiveProfile(profile.id);
                            } catch (error) {
                              DebugConsole.warn(
                                'switchIdentityAndRestartManager failed for '
                                '${profile.id}: $error; '
                                'frontend active profile left unchanged',
                              );
                            }
                          },
                          noSplash: true,
                          disabledColor:
                              profilesController.activeProfile == profile.id &&
                                      stateController.blockAudioChanges
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
                          // atomic identity swap the Set Active button uses;
                          // both must be gated by `blockAudioChanges` so the
                          // backend identity switch + manager restart cannot
                          // race an in-flight call. Deleting a non-active
                          // profile is safe during a call because it touches
                          // neither the call slot nor the active identity.
                          onPressed: stateController.blockAudioChanges &&
                                  profilesController.activeProfile == profile.id
                              ? null
                              : () {
                                  showDialog(
                                      context: listContext,
                                      builder: (BuildContext dialogContext) {
                                        return AlertDialog(
                                          title: const Text('Delete Profile'),
                                          content: const Text(
                                              'Are you sure you want to delete this profile?'),
                                          actions: [
                                            Button(
                                              text: 'Cancel',
                                              onPressed: () {
                                                Navigator.of(dialogContext)
                                                    .pop();
                                              },
                                            ),
                                            Button(
                                              text: 'Delete',
                                              onPressed: () async {
                                                final navigator =
                                                    Navigator.of(dialogContext);
                                                final previousActive =
                                                    profilesController
                                                        .activeProfile;
                                                final wasActive =
                                                    previousActive ==
                                                        profile.id;
                                                Profile? replacement;
                                                if (wasActive) {
                                                  replacement =
                                                      _replacementProfileAfter(
                                                    profilesController,
                                                    excludeId: profile.id,
                                                  );
                                                }
                                                try {
                                                  // Sync the replacement
                                                  // identity first (atomic
                                                  // backend op). Only after
                                                  // it succeeds do we commit
                                                  // the frontend deletion,
                                                  // which switches the
                                                  // active profile to the
                                                  // same replacement.
                                                  if (wasActive &&
                                                      replacement != null) {
                                                    await telepathy
                                                        .switchIdentityAndRestartManager(
                                                      key: replacement.keypair,
                                                    );
                                                  }
                                                  await profilesController
                                                      .removeProfile(
                                                          profile.id);
                                                  if (mounted) {
                                                    navigator.pop();
                                                  }
                                                } catch (error) {
                                                  // Backend rejected or
                                                  // removeProfile failed.
                                                  // Restore the frontend to
                                                  // the prior active profile
                                                  // so the UI stays
                                                  // consistent with whatever
                                                  // identity the backend is
                                                  // actually running.
                                                  DebugConsole.warn(
                                                    'delete of profile '
                                                    '${profile.id} failed: '
                                                    '$error; restoring active '
                                                    'profile to $previousActive',
                                                  );
                                                  if (wasActive &&
                                                      profilesController
                                                              .activeProfile !=
                                                          previousActive &&
                                                      profilesController
                                                          .profiles
                                                          .containsKey(
                                                              previousActive)) {
                                                    await profilesController
                                                        .setActiveProfile(
                                                            previousActive);
                                                  }
                                                  if (mounted) {
                                                    navigator.pop();
                                                  }
                                                }
                                              },
                                            )
                                          ],
                                        );
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
              onPressed: () {
                _profileNameError = null;
                showDialog(
                    context: context,
                    builder: (BuildContext context) {
                      return StatefulBuilder(
                        builder:
                            (BuildContext context, StateSetter setDialogState) {
                          return CallbackShortcuts(
                            bindings: <ShortcutActivator, VoidCallback>{
                              const SingleActivator(LogicalKeyboardKey.enter):
                                  () => _createProfile(context,
                                      profilesController, setDialogState),
                            },
                            child: SimpleDialog(
                              title: const Text('Create Profile'),
                              contentPadding: const EdgeInsets.only(
                                  bottom: 25, left: 25, right: 25),
                              titlePadding: const EdgeInsets.only(
                                  top: 25, left: 25, right: 25, bottom: 15),
                              children: [
                                TextField(
                                  decoration:
                                      _profileNameInputDecoration(context),
                                  controller: _profileNameInput,
                                  onChanged: (_) {
                                    if (_profileNameError == null) return;

                                    setDialogState(() {
                                      _profileNameError = null;
                                    });
                                  },
                                  onSubmitted: (_) => _createProfile(context,
                                      profilesController, setDialogState),
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

/// Returns the [Profile] that would become active if [excludeId] were removed
/// from [controller], mirroring the fallback [_removeProfile] applies inside
/// [ProfilesController]: the first remaining profile by insertion order.
///
/// Returns `null` when [controller] has no other profile to promote; in that
/// case [_removeProfile] itself creates a fresh default profile and the
/// frontend has no pre-existing identity to swap into the backend.
Profile? _replacementProfileAfter(
  ProfilesController controller, {
  required String excludeId,
}) {
  for (final MapEntry<String, Profile> entry in controller.profiles.entries) {
    if (entry.key != excludeId) {
      return entry.value;
    }
  }
  return null;
}
