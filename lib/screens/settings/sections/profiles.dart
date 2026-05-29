import 'dart:core';
import 'package:flutter/services.dart';
import 'package:flutter/material.dart' hide Overlay;
import 'package:flutter_svg/flutter_svg.dart';
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/core/rust/flutter.dart';
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
                          disabled: stateController.isCallActive ||
                              profilesController.activeProfile == profile.id,
                          onPressed: () async {
                            await profilesController
                                .setActiveProfile(profile.id);
                            await telepathy.setIdentity(key: profile.keypair);
                            await telepathy.restartManager();
                          },
                          noSplash: true,
                          disabledColor:
                              profilesController.activeProfile == profile.id &&
                                      stateController.isCallActive
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
                          onPressed: () {
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
                                          Navigator.of(dialogContext).pop();
                                        },
                                      ),
                                      Button(
                                        text: 'Delete',
                                        onPressed: () {
                                          profilesController
                                              .removeProfile(profile.id);
                                          Navigator.of(dialogContext).pop();
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
