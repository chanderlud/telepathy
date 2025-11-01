import 'dart:async';
import 'dart:io';
import 'package:flutter/foundation.dart' show kIsWeb;
import 'package:flutter/material.dart' hide Overlay;

import 'package:telepathy/settings/view.dart';
import 'package:telepathy/src/rust/api/telepathy.dart';
import 'package:telepathy/src/rust/api/flutter.dart';
import 'package:telepathy/src/rust/api/error.dart';
import 'package:telepathy/src/rust/api/audio/player.dart';
import 'package:telepathy/src/rust/api/overlay/overlay.dart';
import 'package:telepathy/src/rust/frb_generated.dart';
import 'package:telepathy/settings/controller.dart';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/services.dart';
import 'package:flutter_secure_storage/flutter_secure_storage.dart';
import 'package:flutter_svg/svg.dart';
import 'package:path_provider/path_provider.dart';
import 'package:permission_handler/permission_handler.dart';
import 'package:process_run/process_run.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:super_clipboard/super_clipboard.dart';

import 'audio_level.dart';
import 'console.dart';

final GlobalKey<NavigatorState> navigatorKey = GlobalKey<NavigatorState>();
SoundHandle? outgoingSoundHandle;

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  await RustLib.init();

  // get logs from rust
  rustSetUp();
  createLogStream().listen((message) {
    DebugConsole.log(message);
  });

  if (!kIsWeb) {
    if (Platform.isAndroid || Platform.isIOS) {
      PermissionStatus status = await Permission.microphone.request();

      if (!status.isGranted) {
        DebugConsole.error('Microphone permission not accepted');
      }
    }
  } else {
    PermissionStatus status = await Permission.microphone.request();

    if (!status.isGranted) {
      DebugConsole.error('Microphone permission not accepted');
    }
  }

  const storage = FlutterSecureStorage();
  final SharedPreferences options = await SharedPreferences.getInstance();

  final SettingsController settingsController =
      SettingsController(storage: storage, options: options);
  await settingsController.init();

  final StateController stateController = StateController();
  final StatisticsController statisticsController = StatisticsController();

  final Overlay overlay = await Overlay.newInstance(
    enabled: settingsController.overlayConfig.enabled,
    x: settingsController.overlayConfig.x.round(),
    y: settingsController.overlayConfig.y.round(),
    width: settingsController.overlayConfig.width.round(),
    height: settingsController.overlayConfig.height.round(),
    fontHeight: settingsController.overlayConfig.fontHeight,
    backgroundColor:
        settingsController.overlayConfig.backgroundColor.toARGB32(),
    fontColor: settingsController.overlayConfig.fontColor.toARGB32(),
  );

  final soundPlayer = SoundPlayer(outputVolume: settingsController.soundVolume);
  soundPlayer.updateOutputDevice(name: settingsController.outputDevice);
  soundPlayer.updateOutputVolume(volume: settingsController.soundVolume);

  ArcHost host = soundPlayer.host();

  final chatStateController = ChatStateController(soundPlayer);

  final telepathy = await Telepathy.newInstance(
      identity: settingsController.keypair,
      host: host,
      networkConfig: settingsController.networkConfig,
      screenshareConfig: settingsController.screenshareConfig,
      overlay: overlay,
      codecConfig: settingsController.codecConfig,
      // called when there is an incoming call
      acceptCall: (String id, Uint8List? ringtone, DartNotify cancel) async {
        Contact? contact = settingsController.getContact(id);

        if (stateController.isCallActive) {
          return false;
        } else if (contact == null) {
          DebugConsole.warn('contact is null');
          return false;
        }

        List<int> bytes;

        if (ringtone == null) {
          bytes = await readSeaBytes('incoming');
        } else {
          bytes = ringtone;
        }

        SoundHandle handle = await soundPlayer.play(bytes: bytes);

        if (navigatorKey.currentState == null ||
            !navigatorKey.currentState!.mounted) {
          return false;
        }

        Future acceptedFuture =
            acceptCallPrompt(navigatorKey.currentState!.context, contact);
        Future cancelFuture = cancel.notified();

        final result = await Future.any([acceptedFuture, cancelFuture]);

        handle.cancel();

        if (result == null) {
          DebugConsole.debug('cancelled');

          if (navigatorKey.currentState != null &&
              navigatorKey.currentState!.mounted) {
            Navigator.pop(navigatorKey.currentState!.context);
          }

          return false; // cancelled
        } else if (result) {
          stateController.setStatus('Connecting');
          stateController.setActiveContact(contact);
        }

        return result;
      },
      // called when a call ends
      callEnded: (String message, bool remote) async {
        if (!stateController.isCallActive) {
          DebugConsole.warn("call ended entered but there is no active call");
          return;
        }

        outgoingSoundHandle?.cancel();
        stateController.endOfCall();

        List<int> bytes = await readSeaBytes('call_ended');
        await soundPlayer.play(bytes: bytes);

        if (message.isNotEmpty &&
            navigatorKey.currentState != null &&
            navigatorKey.currentState!.mounted) {
          showErrorDialog(navigatorKey.currentState!.context,
              remote ? 'Call failed (remote)' : 'Call failed', message);
        }
      },
      // called when a contact is needed in the backend
      getContact: (Uint8List peerId) {
        try {
          Contact? contact = settingsController.contacts.values
              .firstWhere((Contact contact) => contact.idEq(id: peerId));
          return contact.pubClone();
        } catch (_) {
          return null;
        }
      },
      // called when the call connects, disconnects, or reconnects
      callState: (bool disconnected) async {
        if (!stateController.isCallActive) {
          return;
        }

        // ensure the outgoing sound has been canceled as the call is now active
        outgoingSoundHandle?.cancel();
        List<int> bytes;

        if (disconnected && !stateController.callDisconnected) {
          // handles disconnects in an active call
          bytes = await readSeaBytes('disconnected');
          stateController.setStatus('Reconnecting');
          stateController.callDisconnected = true;
        } else if (!disconnected && stateController.callDisconnected) {
          // handles reconnects in an active call
          bytes = await readSeaBytes('reconnected');
          stateController.setStatus('Active');
          stateController.callDisconnected = false;
        } else if (!disconnected && !stateController.callDisconnected) {
          // handles the initial connect
          bytes = await readSeaBytes('connected');
          stateController.setStatus('Active');
          stateController.callDisconnected = false;
        } else {
          return;
        }

        await soundPlayer.play(bytes: bytes);
      },
      // called when a session changes status
      sessionStatus: stateController.updateSession,
      // called when the backend wants to start sessions
      startSessions: (Telepathy telepathy) {
        for (Contact contact in settingsController.contacts.values) {
          telepathy.startSession(contact: contact);
        }
      },
      // called when the backend has updated statistics
      statistics: statisticsController.setStatistics,
      // called when a new chat message is received by the backend
      messageReceived: chatStateController.messageReceived,
      // called when the session manager state changes
      managerActive: (bool active, bool restartable) {
        stateController.setSessionManager(active, restartable);
      },
      screenshareStarted: stateController.screenshareStarted);

  final audioDevices = AudioDevices(telepathy: telepathy);

  // apply options to the instance
  telepathy.setRmsThreshold(decimal: settingsController.inputSensitivity);
  telepathy.setInputVolume(decibel: settingsController.inputVolume);
  telepathy.setOutputVolume(decibel: settingsController.outputVolume);
  telepathy.setDenoise(denoise: settingsController.useDenoise);
  telepathy.setPlayCustomRingtones(
      play: settingsController.playCustomRingtones);
  telepathy.setInputDevice(device: settingsController.inputDevice);
  telepathy.setOutputDevice(device: settingsController.outputDevice);
  telepathy.setSendCustomRingtone(
      send: settingsController.customRingtoneFile != null);
  telepathy.setEfficiencyMode(enabled: settingsController.efficiencyMode);

  if (settingsController.denoiseModel != null) {
    updateDenoiseModel(settingsController.denoiseModel!, telepathy);
  }

  final InterfaceController interfaceController =
      InterfaceController(options: options);
  interfaceController.init();

  runApp(TelepathyApp(
    telepathy: telepathy,
    settingsController: settingsController,
    interfaceController: interfaceController,
    callStateController: stateController,
    player: soundPlayer,
    chatStateController: chatStateController,
    statisticsController: statisticsController,
    overlay: overlay,
    audioDevices: audioDevices,
  ));
}

/// The main app
class TelepathyApp extends StatelessWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final InterfaceController interfaceController;
  final StateController callStateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final ChatStateController chatStateController;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const TelepathyApp(
      {super.key,
      required this.telepathy,
      required this.settingsController,
      required this.callStateController,
      required this.player,
      required this.chatStateController,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices,
      required this.interfaceController});

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
        listenable: interfaceController,
        builder: (BuildContext context, Widget? child) {
          return MaterialApp(
            navigatorKey: navigatorKey,
            theme: ThemeData(
              dialogTheme: const DialogThemeData(
                surfaceTintColor: Color(0xFF27292A),
              ),
              sliderTheme: SliderThemeData(
                showValueIndicator: ShowValueIndicator.onDrag,
                overlayColor: Colors.transparent,
                trackShape: CustomTrackShape(),
                inactiveTrackColor: const Color(0xFF121212),
                activeTrackColor: Color(interfaceController.primaryColor),
              ),
              colorScheme: ColorScheme.dark(
                // primary: Color(0xFF7458ff),
                // secondary: Color(0xFF6950e8),
                primary: Color(interfaceController.primaryColor),
                secondary: Color(interfaceController.secondaryColor),
                brightness: Brightness.dark,
                surface: const Color(0xFF222425),
                secondaryContainer: const Color(0xFF191919),
                tertiaryContainer: const Color(0xFF27292A),
                surfaceDim: const Color(0xFF121212),
              ),
              switchTheme: SwitchThemeData(
                trackOutlineWidth: WidgetStateProperty.all(0),
                trackOutlineColor: WidgetStateProperty.all(Colors.transparent),
                overlayColor: WidgetStateProperty.all(Colors.transparent),
                thumbColor:
                    WidgetStateProperty.all(Theme.of(context).tabBarTheme.indicatorColor),
              ),
              dropdownMenuTheme: DropdownMenuThemeData(
                menuStyle: MenuStyle(
                  backgroundColor:
                      WidgetStateProperty.all(const Color(0xFF191919)),
                  surfaceTintColor:
                      WidgetStateProperty.all(const Color(0xFF191919)),
                ),
              ),
            ),
            home: HomePage(
              telepathy: telepathy,
              settingsController: settingsController,
              interfaceController: interfaceController,
              stateController: callStateController,
              player: player,
              chatStateController: chatStateController,
              statisticsController: statisticsController,
              overlay: overlay,
              audioDevices: audioDevices,
            ),
          );
        });
  }
}

/// The main body of the app
class HomePage extends StatelessWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final InterfaceController interfaceController;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final ChatStateController chatStateController;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const HomePage(
      {super.key,
      required this.telepathy,
      required this.settingsController,
      required this.interfaceController,
      required this.stateController,
      required this.player,
      required this.chatStateController,
      required this.statisticsController,
      required this.overlay,
      required this.audioDevices});

  @override
  Widget build(BuildContext context) {
    ListenableBuilder contactForm = ListenableBuilder(
        listenable: stateController,
        builder: (BuildContext context, Widget? child) {
          if (stateController.isCallActive) {
            return CallDetailsWidget(
                statisticsController: statisticsController,
                stateController: stateController);
          } else {
            return ContactForm(
                telepathy: telepathy, settingsController: settingsController);
          }
        });

    PeriodicNotifier notifier = PeriodicNotifier();

    CallControls callControls = CallControls(
      telepathy: telepathy,
      settingsController: settingsController,
      interfaceController: interfaceController,
      stateController: stateController,
      statisticsController: statisticsController,
      player: player,
      notifier: notifier,
      overlay: overlay,
      audioDevices: audioDevices,
    );

    ChatWidget chatWidget = ChatWidget(
        telepathy: telepathy,
        stateController: stateController,
        chatStateController: chatStateController,
        player: player,
        settingsController: settingsController);

    return Scaffold(
      body: Padding(
          padding: const EdgeInsets.all(20.0),
          child: SafeArea(
              bottom: false,
              child: LayoutBuilder(
                  builder: (BuildContext context, BoxConstraints constraints) {
                ListenableBuilder contactsList = ListenableBuilder(
                    listenable: settingsController,
                    builder: (BuildContext context, Widget? child) {
                      return ListenableBuilder(
                          listenable: stateController,
                          builder: (BuildContext context, Widget? child) {
                            List<Contact> contacts =
                                settingsController.contacts.values.toList();

                            // sort contacts by session status then nickname
                            contacts.sort((a, b) {
                              String aStatus = stateController.sessionStatus(a);
                              String bStatus = stateController.sessionStatus(b);

                              if (aStatus == bStatus) {
                                return a.nickname().compareTo(b.nickname());
                              } else if (aStatus == 'Connected') {
                                return -1;
                              } else if (bStatus == 'Connected') {
                                return 1;
                              } else if (aStatus == 'Connecting') {
                                return -1;
                              } else if (bStatus == 'Connecting') {
                                return 1;
                              } else {
                                return 0;
                              }
                            });

                            return ContactsList(
                              telepathy: telepathy,
                              contacts: contacts,
                              stateController: stateController,
                              settingsController: settingsController,
                              player: player,
                              maxWidth: constraints.maxWidth,
                            );
                          });
                    });
                if (constraints.maxWidth > 600) {
                  return Column(
                    children: [
                      Container(
                        constraints: const BoxConstraints(maxHeight: 250),
                        child: Row(mainAxisSize: MainAxisSize.min, children: [
                          Container(
                            constraints: const BoxConstraints(maxWidth: 300),
                            child: contactForm,
                          ),
                          const SizedBox(width: 20),
                          Flexible(fit: FlexFit.loose, child: contactsList)
                        ]),
                      ),
                      const SizedBox(height: 20),
                      Flexible(
                          fit: FlexFit.loose,
                          child: Row(mainAxisSize: MainAxisSize.min, children: [
                            Container(
                                constraints:
                                    const BoxConstraints(maxWidth: 260),
                                decoration: BoxDecoration(
                                  color: Theme.of(context)
                                      .colorScheme
                                      .tertiaryContainer,
                                  borderRadius: BorderRadius.circular(10.0),
                                ),
                                child: callControls),
                            const SizedBox(width: 20),
                            Flexible(
                                fit: FlexFit.loose,
                                child: Container(
                                    decoration: BoxDecoration(
                                      color: Theme.of(context)
                                          .colorScheme
                                          .secondaryContainer,
                                      borderRadius: BorderRadius.circular(10.0),
                                    ),
                                    padding: const EdgeInsets.only(
                                        left: 10,
                                        right: 10,
                                        top: 5,
                                        bottom: 10),
                                    child: chatWidget))
                          ])),
                    ],
                  );
                } else {
                  return Column(children: [
                    Container(
                      constraints: BoxConstraints(
                          maxHeight: 250, maxWidth: constraints.maxWidth),
                      child: contactsList,
                    ),
                    const SizedBox(height: 20),
                    HomeTabView(
                        widgetOne: callControls,
                        widgetTwo: Padding(
                            padding: const EdgeInsets.only(
                                left: 10, right: 10, top: 5, bottom: 10),
                            child: chatWidget),
                        colorOne:
                            Theme.of(context).colorScheme.tertiaryContainer,
                        colorTwo:
                            Theme.of(context).colorScheme.secondaryContainer,
                        iconOne: const Icon(Icons.call),
                        iconTwo: const Icon(Icons.chat))
                  ]);
                }
              }))),
    );
  }
}

/// A two-widget tab view used to display the call controls and chat widget in a single column
class HomeTabView extends StatefulWidget {
  final Widget widgetOne;
  final Widget widgetTwo;
  final Color colorOne;
  final Color colorTwo;
  final Icon iconOne;
  final Icon iconTwo;

  const HomeTabView(
      {super.key,
      required this.widgetOne,
      required this.widgetTwo,
      required this.colorOne,
      required this.colorTwo,
      required this.iconOne,
      required this.iconTwo});

  @override
  State<HomeTabView> createState() => HomeTabViewState();
}

class HomeTabViewState extends State<HomeTabView>
    with SingleTickerProviderStateMixin {
  late TabController _tabController;
  late Color _backgroundColor = widget.colorOne;

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: 2, vsync: this);
    _tabController.addListener(() {
      setState(() {
        _backgroundColor =
            _tabController.index == 0 ? widget.colorOne : widget.colorTwo;
      });
    });
  }

  @override
  void dispose() {
    _tabController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 2,
      child: Flexible(
        fit: FlexFit.loose,
        child: Container(
          decoration: BoxDecoration(
            color: _backgroundColor,
            borderRadius: BorderRadius.circular(10.0),
          ),
          child: Column(
            children: [
              Container(
                decoration: BoxDecoration(
                  color: Theme.of(context).colorScheme.secondaryContainer,
                  borderRadius:
                      const BorderRadius.vertical(top: Radius.circular(10.0)),
                ),
                padding: const EdgeInsets.symmetric(vertical: 12),
                child: TabBar(
                  controller: _tabController,
                  splashFactory: NoSplash.splashFactory,
                  overlayColor: WidgetStateProperty.all(Colors.transparent),
                  dividerHeight: 0,
                  padding: const EdgeInsets.all(0),
                  tabs: [
                    widget.iconOne,
                    widget.iconTwo,
                  ],
                ),
              ),
              Flexible(
                child: TabBarView(
                  controller: _tabController,
                  children: [
                    widget.widgetOne,
                    widget.widgetTwo,
                  ],
                ),
              )
            ],
          ),
        ),
      ),
    );
  }
}

/// A widget which allows the user to add a contact
class ContactForm extends StatefulWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;

  const ContactForm(
      {super.key, required this.telepathy, required this.settingsController});

  @override
  State<ContactForm> createState() => ContactFormState();
}

/// The state for ContactForm
class ContactFormState extends State<ContactForm> {
  final TextEditingController _nicknameInput = TextEditingController();
  final TextEditingController _peerIdInput = TextEditingController();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text("Add Contact", style: TextStyle(fontSize: 20)),
          const SizedBox(height: 21),
          TextInput(controller: _nicknameInput, labelText: 'Nickname'),
          const SizedBox(height: 15),
          TextInput(
              controller: _peerIdInput,
              labelText: 'Peer ID',
              hintText: 'string encoded peer ID',
              obscureText: true),
          const SizedBox(height: 26),
          Center(
            child: Button(
              text: 'Add Contact',
              onPressed: () async {
                String nickname = _nicknameInput.text;
                String peerId = _peerIdInput.text;

                if (nickname.isEmpty || peerId.isEmpty) {
                  showErrorDialog(context, 'Failed to add contact',
                      'Nickname and peer id cannot be empty');
                  return;
                } else if (widget.settingsController.contacts.keys
                    .contains(peerId)) {
                  showErrorDialog(context, 'Failed to add contact',
                      'Contact for peer ID already exists');
                  return;
                } else if (widget.settingsController.peerId == peerId) {
                  showErrorDialog(context, 'Failed to add contact',
                      'Cannot add self as a contact');
                  return;
                }

                try {
                  Contact contact =
                      widget.settingsController.addContact(nickname, peerId);

                  widget.telepathy.startSession(contact: contact);

                  _nicknameInput.clear();
                  _peerIdInput.clear();
                } on DartError catch (_) {
                  showErrorDialog(
                      context, 'Failed to add contact', 'Invalid peer ID');
                }
              },
            ),
          ),
        ],
      ),
    );
  }
}

/// A widget which displays a list of ContactWidgets
class ContactsList extends StatelessWidget {
  final Telepathy telepathy;
  final StateController stateController;
  final SettingsController settingsController;
  final List<Contact> contacts;
  final SoundPlayer player;
  final double maxWidth;

  const ContactsList(
      {super.key,
      required this.telepathy,
      required this.contacts,
      required this.stateController,
      required this.settingsController,
      required this.player,
      required this.maxWidth});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.only(bottom: 15, left: 12, right: 12, top: 8),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Padding(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 7),
              child: Row(
                children: [
                  const Text("Contacts", style: TextStyle(fontSize: 20)),
                  if (maxWidth <= 600)
                    Padding(
                      padding: const EdgeInsets.only(left: 10, top: 3),
                      child: IconButton(
                          onPressed: () {
                            showDialog(
                                context: context,
                                builder: (BuildContext context) {
                                  return SimpleDialog(
                                    backgroundColor: Theme.of(context)
                                        .colorScheme
                                        .secondaryContainer,
                                    children: [
                                      ContactForm(
                                        telepathy: telepathy,
                                        settingsController: settingsController,
                                      )
                                    ],
                                  );
                                });
                          },
                          visualDensity: const VisualDensity(
                              horizontal: -4.0, vertical: -4.0),
                          icon: SvgPicture.asset('assets/icons/Plus.svg')),
                    ),
                ],
              )),
          const SizedBox(height: 10.0),
          Flexible(
            fit: FlexFit.loose,
            child: Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(10),
              ),
              child: ListView.builder(
                itemCount: contacts.length,
                itemBuilder: (BuildContext context, int index) {
                  return ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) {
                        return ContactWidget(
                          contact: contacts[index],
                          telepathy: telepathy,
                          stateController: stateController,
                          player: player,
                          settingsController: settingsController,
                        );
                      });
                },
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// A widget which displays a single contact
class ContactWidget extends StatefulWidget {
  final Contact contact;
  final Telepathy telepathy;
  final StateController stateController;
  final SettingsController settingsController;
  final SoundPlayer player;

  const ContactWidget(
      {super.key,
      required this.contact,
      required this.telepathy,
      required this.stateController,
      required this.player,
      required this.settingsController});

  @override
  State<StatefulWidget> createState() => ContactWidgetState();
}

class ContactWidgetState extends State<ContactWidget> {
  bool isHovered = false;
  late TextEditingController _nicknameInput;

  @override
  void initState() {
    super.initState();
    _nicknameInput = TextEditingController(text: widget.contact.nickname());
  }

  @override
  void didUpdateWidget(ContactWidget oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (widget.contact != oldWidget.contact) {
      _nicknameInput.text = widget.contact.nickname();
    }
  }

  @override
  void dispose() {
    _nicknameInput.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    bool online = widget.stateController.isOnlineContact(widget.contact);
    bool active = widget.stateController.isActiveContact(widget.contact);
    String status =
        widget.stateController.sessions[widget.contact.peerId()] ?? 'Unknown';

    return InkWell(
      onHover: (hover) {
        setState(() {
          isHovered = hover;
        });
      },
      onTap: () {
        showDialog(
            barrierDismissible: false,
            context: context,
            builder: (BuildContext context) {
              return SimpleDialog(
                title: Row(
                  mainAxisAlignment: MainAxisAlignment.spaceBetween,
                  children: [
                    const Text('Edit Contact'),
                    IconButton(
                      onPressed: () async {
                        if (!widget.stateController
                            .isActiveContact(widget.contact)) {
                          bool confirm = await showDialog<bool>(
                                  context: context,
                                  builder: (BuildContext context) {
                                    return SimpleDialog(
                                      title: const Text('Warning'),
                                      contentPadding: const EdgeInsets.only(
                                          bottom: 25, left: 25, right: 25),
                                      titlePadding: const EdgeInsets.only(
                                          top: 25,
                                          left: 25,
                                          right: 25,
                                          bottom: 20),
                                      children: [
                                        const Text(
                                            'Are you sure you want to delete this contact?'),
                                        const SizedBox(height: 20),
                                        Row(
                                          mainAxisAlignment:
                                              MainAxisAlignment.end,
                                          children: [
                                            Button(
                                              text: 'Cancel',
                                              onPressed: () {
                                                Navigator.pop(context, false);
                                              },
                                            ),
                                            const SizedBox(width: 10),
                                            Button(
                                              text: 'Delete',
                                              onPressed: () {
                                                Navigator.pop(context, true);
                                              },
                                            ),
                                          ],
                                        ),
                                      ],
                                    );
                                  }) ??
                              false;

                          if (confirm) {
                            widget.settingsController
                                .removeContact(widget.contact);
                            widget.telepathy
                                .stopSession(contact: widget.contact);
                            widget.settingsController.saveContacts();
                          }

                          if (context.mounted) {
                            Navigator.pop(context);
                          }
                        } else {
                          showErrorDialog(context, 'Warning',
                              'Cannot delete a contact while in an active call');
                        }
                      },
                      icon: SvgPicture.asset('assets/icons/Trash.svg',
                          semanticsLabel: 'Delete contact icon'),
                    ),
                  ],
                ),
                contentPadding:
                    const EdgeInsets.only(bottom: 25, left: 25, right: 25),
                titlePadding: const EdgeInsets.only(
                    top: 25, left: 25, right: 25, bottom: 20),
                children: [
                  TextInput(
                      enabled: !widget.stateController
                          .isActiveContact(widget.contact),
                      controller: _nicknameInput,
                      labelText: 'Nickname',
                      onChanged: (value) {
                        widget.contact.setNickname(nickname: value);
                      }),
                  const SizedBox(height: 20),
                  Button(
                    text: 'Save',
                    onPressed: () {
                      widget.settingsController.saveContacts();
                      Navigator.pop(context);
                    },
                  ),
                ],
              );
            });
      },
      hoverColor: Colors.transparent,
      child: Container(
        margin: const EdgeInsets.all(5.0),
        decoration: BoxDecoration(
          color: Theme.of(context).colorScheme.secondaryContainer,
          borderRadius: BorderRadius.circular(10.0),
        ),
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 6.5),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            CircleAvatar(
              maxRadius: 17,
              // TODO the edit icon needs some work
              child: SvgPicture.asset(isHovered
                  ? 'assets/icons/Edit.svg'
                  : 'assets/icons/Profile.svg'),
            ),
            const SizedBox(width: 10),
            Text(widget.contact.nickname(),
                style: const TextStyle(fontSize: 16)),
            const Spacer(),
            if (status == 'Inactive')
              IconButton(
                  onPressed: () {
                    widget.telepathy.startSession(contact: widget.contact);
                  },
                  icon: SvgPicture.asset('assets/icons/Restart.svg',
                      semanticsLabel: 'Retry the session initiation')),
            if (status == 'Inactive') const SizedBox(width: 4),
            if (status == 'Connecting')
              const Padding(
                padding: EdgeInsets.symmetric(vertical: 10),
                child: SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(strokeWidth: 3)),
              ),
            if (status == 'Connecting') const SizedBox(width: 10),
            if (!online && status != 'Connecting')
              Padding(
                  padding: const EdgeInsets.only(left: 7, right: 10),
                  child: SvgPicture.asset(
                    'assets/icons/Offline.svg',
                    semanticsLabel: 'Offline icon',
                    width: 26,
                  )),
            if (active)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/PhoneOff.svg',
                  semanticsLabel: 'End call icon',
                  width: 32,
                ),
                onPressed: () async {
                  outgoingSoundHandle?.cancel();

                  widget.telepathy.endCall();
                  widget.stateController.endOfCall();

                  List<int> bytes = await readSeaBytes('call_ended');
                  await widget.player.play(bytes: bytes);
                },
              ),
            if (!active && online)
              IconButton(
                visualDensity: VisualDensity.comfortable,
                icon: SvgPicture.asset(
                  'assets/icons/Phone.svg',
                  semanticsLabel: 'Call icon',
                  width: 32,
                ),
                onPressed: () async {
                  if (widget.stateController.isCallActive) {
                    showErrorDialog(context, 'Call failed',
                        'There is a call already active');
                    return;
                  } else if (widget.stateController.inAudioTest) {
                    showErrorDialog(context, 'Call failed',
                        'Cannot make a call while in an audio test');
                    return;
                  } else if (widget.stateController.callEndedRecently) {
                    // if the call button is pressed right after a call ended, we assume the user did not want to make a call
                    return;
                  }

                  widget.stateController.setStatus('Connecting');
                  List<int> bytes = await readSeaBytes('outgoing');
                  outgoingSoundHandle = await widget.player.play(bytes: bytes);

                  try {
                    await widget.telepathy.startCall(contact: widget.contact);
                    widget.stateController.setActiveContact(widget.contact);
                  } on DartError catch (e) {
                    widget.stateController.setStatus('Inactive');
                    outgoingSoundHandle?.cancel();
                    if (!context.mounted) return;
                    showErrorDialog(context, 'Call failed', e.message);
                  }
                },
              )
          ],
        ),
      ),
    );
  }
}

/// A widget with commonly used controls for a call
class CallControls extends StatelessWidget {
  final Telepathy telepathy;
  final SettingsController settingsController;
  final InterfaceController interfaceController;
  final StateController stateController;
  final StatisticsController statisticsController;
  final SoundPlayer player;
  final PeriodicNotifier notifier;
  final Overlay overlay;
  final AudioDevices audioDevices;

  const CallControls(
      {super.key,
      required this.telepathy,
      required this.settingsController,
      required this.stateController,
      required this.player,
      required this.statisticsController,
      required this.notifier,
      required this.overlay,
      required this.audioDevices,
      required this.interfaceController});

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        const SizedBox(height: 10),
        ListenableBuilder(
            listenable: stateController,
            builder: (BuildContext context, Widget? child) {
              Widget body;

              if (stateController.sessionManagerActive) {
                if (stateController.isCallActive) {
                  body = ListenableBuilder(
                      listenable: notifier,
                      builder: (BuildContext context, Widget? child) {
                        return Text(stateController.callDuration,
                            style: const TextStyle(fontSize: 20));
                      });
                } else {
                  body = Text(stateController.status,
                      style: const TextStyle(fontSize: 20));
                }
              } else {
                body = Row(
                  mainAxisAlignment: MainAxisAlignment.center,
                  children: [
                    const SizedBox(width: 15),
                    const Text('Session Manager Inactive',
                        style:
                            TextStyle(fontSize: 16, color: Color(0xFFdc2626))),
                    stateController.sessionManagerRestartable
                        ? const Spacer()
                        : const SizedBox(width: 10),
                    stateController.sessionManagerRestartable
                        ? IconButton(
                            onPressed: () {
                              telepathy.restartManager();
                            },
                            icon: SvgPicture.asset('assets/icons/Restart.svg',
                                colorFilter: const ColorFilter.mode(
                                    Color(0xFFdc2626), BlendMode.srcIn),
                                semanticsLabel: 'Restart session manager'))
                        : Container(),
                    const SizedBox(width: 5),
                  ],
                );
              }

              return SizedBox(
                height: 40,
                child: Center(child: body),
              );
            }),
        Padding(
          padding: const EdgeInsets.only(left: 25, right: 25, top: 20),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              const Text('Output Volume', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.outputVolume,
                        onChanged: (value) async {
                          await settingsController.updateOutputVolume(value);
                          telepathy.setOutputVolume(decibel: value);
                        },
                        min: -15,
                        max: 15,
                        label:
                            '${settingsController.outputVolume.toStringAsFixed(2)} db');
                  }),
              const SizedBox(height: 2),
              const Text('Input Volume', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.inputVolume,
                        onChanged: (value) async {
                          await settingsController.updateInputVolume(value);
                          telepathy.setInputVolume(decibel: value);
                        },
                        min: -15,
                        max: 15,
                        label:
                            '${settingsController.inputVolume.toStringAsFixed(2)} db');
                  }),
              const SizedBox(height: 2),
              const Text('Input Sensitivity', style: TextStyle(fontSize: 15)),
              ListenableBuilder(
                  listenable: settingsController,
                  builder: (BuildContext context, Widget? child) {
                    return Slider(
                        value: settingsController.inputSensitivity,
                        onChanged: (value) async {
                          await settingsController
                              .updateInputSensitivity(value);
                          telepathy.setRmsThreshold(decimal: value);
                        },
                        min: -16,
                        max: 50,
                        label:
                            '${settingsController.inputSensitivity.toStringAsFixed(2)} db');
                  }),
            ],
          ),
        ),
        const Spacer(),
        Container(
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.secondaryContainer,
              borderRadius: const BorderRadius.only(
                  bottomLeft: Radius.circular(10.0),
                  bottomRight: Radius.circular(10.0)),
            ),
            child: Padding(
              padding: const EdgeInsets.all(5.0),
              child: Center(
                  child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) {
                        return IconButton(
                            onPressed: () async {
                              if (stateController.isDeafened) {
                                return;
                              }

                              List<int> bytes = stateController.isMuted
                                  ? await readSeaBytes('unmute')
                                  : await readSeaBytes('mute');
                              player.play(bytes: bytes);

                              stateController.mute();
                              telepathy.setMuted(
                                  muted: stateController.isMuted);
                            },
                            icon: SvgPicture.asset(
                                stateController.isDeafened |
                                        stateController.isMuted
                                    ? 'assets/icons/MicrophoneOff.svg'
                                    : 'assets/icons/Microphone.svg',
                                width: 24));
                      }),
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) {
                        return IconButton(
                            onPressed: () async {
                              List<int> bytes = stateController.isDeafened
                                  ? await readSeaBytes('deafen')
                                  : await readSeaBytes('undeafen');
                              player.play(bytes: bytes);

                              stateController.deafen();
                              telepathy.setDeafened(
                                  deafened: stateController.isDeafened);

                              if (stateController.isDeafened &&
                                  stateController.isMuted) {
                                telepathy.setMuted(muted: true);
                              } else {
                                telepathy.setMuted(muted: false);
                              }
                            },
                            visualDensity: VisualDensity.comfortable,
                            icon: SvgPicture.asset(
                                stateController.isDeafened
                                    ? 'assets/icons/SpeakerOff.svg'
                                    : 'assets/icons/Speaker.svg',
                                width: 28));
                      }),
                  IconButton(
                      onPressed: () {
                        Navigator.push(
                            context,
                            MaterialPageRoute(
                              builder: (context) => Scaffold(body:
                                  LayoutBuilder(builder: (BuildContext context,
                                      BoxConstraints constraints) {
                                return SettingsPage(
                                  controller: settingsController,
                                  interfaceController: interfaceController,
                                  telepathy: telepathy,
                                  stateController: stateController,
                                  statisticsController: statisticsController,
                                  player: player,
                                  overlay: overlay,
                                  audioDevices: audioDevices,
                                  constraints: constraints,
                                );
                              })),
                            ));
                      },
                      icon: SvgPicture.asset('assets/icons/Settings.svg')),
                  const SizedBox(width: 1),
                  ListenableBuilder(
                      listenable: stateController,
                      builder: (BuildContext context, Widget? child) =>
                          IconButton(
                              onPressed: () {
                                if (stateController.activeContact == null) {
                                  return;
                                }

                                if (!stateController.isSendingScreenshare) {
                                  telepathy.startScreenshare(
                                      contact: stateController.activeContact!);
                                } else {
                                  stateController.stopScreenshare(true);
                                }
                              },
                              icon: SvgPicture.asset(
                                  stateController.isSendingScreenshare
                                      ? 'assets/icons/PhoneOff.svg'
                                      : 'assets/icons/Screenshare.svg',
                                  semanticsLabel: 'Screenshare icon'))),
                ],
              )),
            ))
      ],
    );
  }
}

class ChatWidget extends StatefulWidget {
  final Telepathy telepathy;
  final StateController stateController;
  final SettingsController settingsController;
  final ChatStateController chatStateController;
  final SoundPlayer player;

  const ChatWidget(
      {super.key,
      required this.telepathy,
      required this.stateController,
      required this.chatStateController,
      required this.player,
      required this.settingsController});

  @override
  State<StatefulWidget> createState() => ChatWidgetState();
}

class ChatWidgetState extends State<ChatWidget> {
  final FocusNode _focusNode = FocusNode();
  final Map<String, bool> _attachmentHovered = {};

  @override
  void initState() {
    super.initState();
    widget.stateController.addListener(_onStateControllerChange);
    ClipboardEvents.instance?.registerPasteEventListener(_onPasteEvent);

    _focusNode.addListener(() {
      if (_focusNode.hasFocus) {
        HardwareKeyboard.instance.addHandler(_onKeyEvent);
      } else {
        HardwareKeyboard.instance.removeHandler(_onKeyEvent);
      }
    });
  }

  @override
  void dispose() {
    widget.stateController.removeListener(_onStateControllerChange);
    ClipboardEvents.instance?.unregisterPasteEventListener(_onPasteEvent);
    _focusNode.dispose();
    super.dispose();
  }

  void sendMessage(String text) async {
    if (!widget.chatStateController.active) return;
    if (text.isEmpty && widget.chatStateController.attachments.isEmpty) return;

    Contact contact = widget.stateController.activeContact!;

    try {
      ChatMessage message = widget.telepathy.buildChat(
          contact: contact,
          text: text,
          attachments: widget.chatStateController.attachments);
      await widget.telepathy.sendChat(message: message);

      message.clearAttachments();
      widget.chatStateController.messages.add(message);
      widget.chatStateController.clearInput();
    } on DartError catch (error) {
      if (!mounted) return;
      showErrorDialog(context, 'Message Send Failed', error.message);
    }
  }

  void _onStateControllerChange() {
    if (widget.stateController.isCallActive ==
        widget.chatStateController.active) {
      return;
    } else if (!widget.stateController.isCallActive &&
        widget.chatStateController.active) {
      widget.chatStateController.clearState();
    }

    setState(() {
      widget.chatStateController.active = widget.stateController.isCallActive;
    });
  }

  Future<void> _onPasteEvent(ClipboardReadEvent event) async {
    ClipboardReader reader = await event.getClipboardReader();
    _handlePaste(reader);
  }

  // TODO mobile compatibility
  Future<void> _onChooseFile() async {
    FilePickerResult? result = await FilePicker.platform.pickFiles();

    if (result != null) {
      File file = File(result.files.single.path!);
      String name = result.files.single.name;

      widget.chatStateController.addAttachmentFile(name, file);
    }
  }

  bool _onKeyEvent(KeyEvent event) {
    if (event is KeyDownEvent) {
      if (HardwareKeyboard.instance.isControlPressed &&
          event.logicalKey == LogicalKeyboardKey.keyV) {
        final clipboard = SystemClipboard.instance;

        if (clipboard != null) {
          clipboard.read().then((reader) => _handlePaste(reader));
          return true;
        } else {
          DebugConsole.debug('Clipboard is null');
        }
      }
    }

    return false;
  }

  Future<void> _handlePaste(ClipboardReader reader) async {
    for (DataReader reader in reader.items) {
      final formats = reader.getFormats(Formats.standardFormats);
      String? suggestedName = await reader.getSuggestedName();

      for (DataFormat format in formats) {
        // TODO handle more formats
        switch (format) {
          // plain text is already handled in the text field
          case Formats.plainText:
            break;
          case Formats.png || Formats.jpeg:
            final image = await reader.readFile(format as FileFormat);
            widget.chatStateController
                .addAttachmentMemory(suggestedName!, image!);
        }
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        Flexible(
            fit: FlexFit.loose,
            child: ListenableBuilder(
              listenable: widget.chatStateController,
              builder: (BuildContext context, Widget? child) {
                return ListView.builder(
                    itemCount: widget.chatStateController.messages.length,
                    itemBuilder: (BuildContext context, int index) {
                      ChatMessage message =
                          widget.chatStateController.messages[index];
                      bool sender = message.isSender(
                          identity: widget.settingsController.peerId);

                      List<(String, Uint8List)> attachments =
                          message.attachments();

                      List<Widget> widgets = attachments.map((attachment) {
                        (File?, Image?)? file =
                            widget.chatStateController.files[attachment.$1];

                        if (file == null) {
                          DebugConsole.debug('Attachment file is null');
                          return Text('Attachment: ${attachment.$1}');
                        } else {
                          if (file.$2 != null) {
                            return Container(
                              width: 500,
                              margin: const EdgeInsets.symmetric(vertical: 5),
                              child: InkWell(
                                hoverColor: Colors.transparent,
                                onTap: () {
                                  showImagePreview(file.$2!);
                                },
                                onSecondaryTapDown: (details) {
                                  showAttachmentMenu(
                                      details.globalPosition, file.$1);
                                },
                                child: ClipRRect(
                                  borderRadius: BorderRadius.circular(5.0),
                                  child: file.$2!,
                                ),
                              ),
                            );
                          } else {
                            // TODO make this a proper attachment widget
                            return InkWell(
                              onSecondaryTapDown: (details) {
                                showAttachmentMenu(
                                    details.globalPosition, file.$1);
                              },
                              child: Text('Attachment: ${attachment.$1}'),
                            );
                          }
                        }
                      }).toList();

                      if (message.text.isNotEmpty) {
                        widgets.insert(
                            0,
                            Container(
                              padding: const EdgeInsets.symmetric(
                                  horizontal: 10, vertical: 5),
                              margin: const EdgeInsets.symmetric(vertical: 5),
                              decoration: BoxDecoration(
                                  color: sender
                                      ? Theme.of(context).colorScheme.secondary
                                      : Theme.of(context)
                                          .colorScheme
                                          .tertiaryContainer,
                                  borderRadius: BorderRadius.only(
                                      topLeft: const Radius.circular(10.0),
                                      topRight: const Radius.circular(10.0),
                                      bottomLeft:
                                          Radius.circular(sender ? 10.0 : 0),
                                      bottomRight:
                                          Radius.circular(sender ? 0 : 10.0))),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                crossAxisAlignment: CrossAxisAlignment.end,
                                children: [
                                  Theme(
                                    data: ThemeData(
                                        textSelectionTheme:
                                            TextSelectionThemeData(
                                      selectionColor:
                                          sender ? Colors.blue : null,
                                    )),
                                    child: SelectableText(
                                      message.text,
                                    ),
                                  ),
                                  const SizedBox(width: 5),
                                  Text(message.time(),
                                      style: TextStyle(
                                          fontSize: 10,
                                          color: sender
                                              ? Colors.white60
                                              : Colors.grey)),
                                ],
                              ),
                            ));
                      }

                      return Align(
                        alignment: sender
                            ? Alignment.centerRight
                            : Alignment.centerLeft,
                        child: Column(
                          crossAxisAlignment: sender
                              ? CrossAxisAlignment.end
                              : CrossAxisAlignment.start,
                          children: widgets,
                        ),
                      );
                    });
              },
            )),
        ListenableBuilder(
            listenable: widget.stateController,
            builder: (BuildContext context, Widget? child) {
              const noBorder = OutlineInputBorder(
                  borderSide: BorderSide(
                color: Colors.transparent,
              ));

              return Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  ListenableBuilder(
                      listenable: widget.chatStateController,
                      builder: (BuildContext context, Widget? child) {
                        List<Widget> attachments = widget
                            .chatStateController.attachments
                            .map((attachment) {
                          return InkWell(
                            mouseCursor: SystemMouseCursors.basic,
                            onTap: () {},
                            onHover: (hovered) {
                              setState(() {
                                _attachmentHovered[attachment.$1] = hovered;
                              });
                            },
                            child: Stack(
                              children: [
                                Container(
                                  decoration: BoxDecoration(
                                    color: Theme.of(context)
                                        .colorScheme
                                        .tertiaryContainer,
                                    borderRadius: BorderRadius.circular(10.0),
                                    border:
                                        Border.all(color: Colors.grey.shade400),
                                  ),
                                  margin:
                                      const EdgeInsets.only(top: 5, right: 5),
                                  child: Padding(
                                      padding: const EdgeInsets.only(
                                          left: 4, right: 4, top: 2, bottom: 4),
                                      child: Text(attachment.$1)),
                                ),
                                if (_attachmentHovered[attachment.$1] ?? false)
                                  Positioned(
                                      right: 0,
                                      child: InkWell(
                                        onTap: () {
                                          widget.chatStateController
                                              .removeAttachment(attachment.$1);
                                        },
                                        child: Container(
                                          decoration: BoxDecoration(
                                            color: Theme.of(context)
                                                .colorScheme
                                                .tertiaryContainer,
                                            borderRadius:
                                                BorderRadius.circular(10.0),
                                          ),
                                          child: SvgPicture.asset(
                                            'assets/icons/Trash.svg',
                                            semanticsLabel:
                                                'Close attachment icon',
                                            colorFilter: const ColorFilter.mode(
                                                Color(0xFFdc2626),
                                                BlendMode.srcIn),
                                            width: 20,
                                          ),
                                        ),
                                      )),
                              ],
                            ),
                          );
                        }).toList();

                        return Wrap(
                          children: attachments,
                        );
                      }),
                  const SizedBox(height: 7),
                  SizedBox(
                    height: 50,
                    child: Container(
                      decoration: BoxDecoration(
                        color: Theme.of(context).colorScheme.tertiaryContainer,
                        borderRadius: BorderRadius.circular(10.0),
                        border: Border.all(
                            color: widget.chatStateController.active
                                ? Colors.grey.shade400
                                : Colors.grey.shade600),
                      ),
                      padding: EdgeInsets.symmetric(
                          horizontal:
                              widget.chatStateController.active ? 4 : 12),
                      child: Row(
                        mainAxisAlignment: MainAxisAlignment.start,
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          if (widget.chatStateController.active)
                            IconButton(
                              onPressed: _onChooseFile,
                              icon: SvgPicture.asset(
                                'assets/icons/Attachment.svg',
                                semanticsLabel: 'Attachment button icon',
                                width: 26,
                              ),
                              hoverColor: Colors.transparent,
                            ),
                          Flexible(
                              fit: FlexFit.loose,
                              child: TextField(
                                focusNode: _focusNode,
                                controller:
                                    widget.chatStateController.messageInput,
                                enabled: widget.chatStateController.active,
                                onSubmitted: (message) {
                                  sendMessage(message);
                                },
                                decoration: InputDecoration(
                                  labelText: widget.chatStateController.active
                                      ? 'Message'
                                      : 'Chat disabled',
                                  floatingLabelBehavior:
                                      FloatingLabelBehavior.never,
                                  disabledBorder: noBorder,
                                  border: noBorder,
                                  focusedBorder: noBorder,
                                  enabledBorder: noBorder,
                                  contentPadding:
                                      const EdgeInsets.symmetric(horizontal: 2),
                                ),
                              )),
                          if (widget.chatStateController.active)
                            IconButton(
                              onPressed: () {
                                String message = widget
                                    .chatStateController.messageInput.text;
                                sendMessage(message);
                              },
                              icon: SvgPicture.asset(
                                'assets/icons/Send.svg',
                                semanticsLabel: 'Send button icon',
                                width: 32,
                              ),
                              hoverColor: Colors.transparent,
                            )
                        ],
                      ),
                    ),
                  ),
                ],
              );
            }),
      ],
    );
  }

  void showAttachmentMenu(Offset position, File? file) {
    showDialog(
      context: context,
      barrierColor: Colors.transparent,
      builder: (BuildContext context) {
        return CustomPositionedDialog(position: position, file: file);
      },
    );
  }

  void showImagePreview(Image image) {
    showDialog(
        context: context,
        builder: (BuildContext context) {
          return GestureDetector(
            onTap: () {
              Navigator.of(context).pop();
            },
            child: Stack(
              children: [
                Center(
                  child: InkWell(
                    onTap: () {},
                    child: image,
                  ),
                ),
              ],
            ),
          );
        });
  }
}

/// Turn [DataReader.getValue] into a future.
extension _ReadValue on DataReader {
  Future<Uint8List?>? readFile(FileFormat format) {
    final c = Completer<Uint8List?>();
    final progress = getFile(format, (file) async {
      try {
        final all = await file.readAll();
        c.complete(all);
      } catch (e) {
        c.completeError(e);
      }
    }, onError: (e) {
      c.completeError(e);
    });
    if (progress == null) {
      c.complete(null);
    }
    return c.future;
  }
}

/// A widget which displays details about the call
class CallDetailsWidget extends StatelessWidget {
  final StatisticsController statisticsController;
  final StateController stateController;

  const CallDetailsWidget(
      {super.key,
      required this.statisticsController,
      required this.stateController});

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(vertical: 15.0, horizontal: 20.0),
      decoration: BoxDecoration(
        color: Theme.of(context).colorScheme.secondaryContainer,
        borderRadius: BorderRadius.circular(10.0),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ListenableBuilder(
              listenable: stateController,
              builder: (BuildContext context, Widget? child) {
                return Text('Call ${stateController.status.toLowerCase()}',
                    style: const TextStyle(fontSize: 20));
              }),
          const SizedBox(height: 8),
          ListenableBuilder(
              listenable: statisticsController,
              builder: (BuildContext context, Widget? child) {
                Color color = getColor(statisticsController.loss);
                return Row(
                  children: [
                    const Text(
                      'Loss: ',
                      style: TextStyle(fontSize: 17),
                    ),
                    Text(
                      '${(statisticsController.loss * 100).toStringAsFixed(1)}%',
                      style: TextStyle(color: color, fontSize: 17),
                    ),
                  ],
                );
              }),
          const Spacer(),
          const Text('Input level'),
          const SizedBox(height: 7),
          ListenableBuilder(
              listenable: statisticsController,
              builder: (BuildContext context, Widget? child) {
                return AudioLevel(
                    level: statisticsController.inputLevel, numRectangles: 20);
              }),
          const SizedBox(height: 9),
          const Text('Output level'),
          const SizedBox(height: 7),
          ListenableBuilder(
              listenable: statisticsController,
              builder: (BuildContext context, Widget? child) {
                return AudioLevel(
                    level: statisticsController.outputLevel, numRectangles: 20);
              }),
          const SizedBox(height: 12),
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              ListenableBuilder(
                  listenable: statisticsController,
                  builder: (BuildContext context, Widget? child) {
                    Color color = getColor(statisticsController.latency / 200);
                    return SvgPicture.asset('assets/icons/Latency.svg',
                        colorFilter: ColorFilter.mode(color, BlendMode.srcIn),
                        semanticsLabel: 'Latency icon');
                  }),
              const SizedBox(width: 7),
              ListenableBuilder(
                  listenable: statisticsController,
                  builder: (BuildContext context, Widget? child) {
                    return Text('${statisticsController.latency} ms',
                        style: const TextStyle(height: 0));
                  }),
              const Spacer(),
              SvgPicture.asset('assets/icons/Upload.svg',
                  semanticsLabel: 'Upload icon'),
              const SizedBox(width: 4),
              ListenableBuilder(
                  listenable: statisticsController,
                  builder: (BuildContext context, Widget? child) {
                    return Text(statisticsController.upload,
                        style: const TextStyle(height: 0));
                  }),
              const Spacer(),
              SvgPicture.asset('assets/icons/Download.svg',
                  semanticsLabel: 'Download icon'),
              const SizedBox(width: 4),
              ListenableBuilder(
                  listenable: statisticsController,
                  builder: (BuildContext context, Widget? child) {
                    return Text(statisticsController.download,
                        style: const TextStyle(height: 0));
                  }),
            ],
          ),
        ],
      ),
    );
  }
}

/// A custom right click dialog
class CustomPositionedDialog extends StatelessWidget {
  final Offset position;
  final File? file;

  const CustomPositionedDialog(
      {super.key, required this.position, required this.file});

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () {
        Navigator.of(context).pop();
      },
      onSecondaryTap: () {
        Navigator.of(context).pop();
      },
      child: Stack(
        children: [
          Positioned(
            left: position.dx,
            top: position.dy,
            child: Container(
              decoration: BoxDecoration(
                color: Theme.of(context).colorScheme.tertiaryContainer,
                borderRadius: BorderRadius.circular(5.0),
              ),
              padding: const EdgeInsets.all(10),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  InkWell(
                    onTap: () async {
                      final clipboard = SystemClipboard.instance;

                      if (clipboard == null) {
                        DebugConsole.warn(
                            'Clipboard not supported on this platform');
                      } else {
                        final item = DataWriterItem();

                        if (file != null) {
                          item.add(Formats.fileUri(Uri(path: file!.path)));
                        } else {
                          DebugConsole.warn('File is null');
                        }

                        clipboard.write([item]);
                      }

                      if (context.mounted) {
                        Navigator.of(context).pop();
                      }
                    },
                    child: const SizedBox(
                      width: 125,
                      child: Text('Copy'),
                    ),
                  ),
                  // TODO need some kind of divider here
                  const SizedBox(height: 5),
                  if ((Platform.isMacOS ||
                          Platform.isLinux ||
                          Platform.isWindows) &&
                      file != null)
                    InkWell(
                      onTap: () {
                        // init shell
                        Shell shell = Shell();

                        // TODO work on cross platform support
                        if (Platform.isWindows) {
                          shell.run(
                              'explorer.exe /select,${file!.path.replaceAll("/", "\\")}');
                        } else if (Platform.isMacOS) {
                          shell.run('open -R "${file!.path}"');
                        } else {
                          DebugConsole.warn(
                              'Opening file in folder not supported on this platform');
                        }

                        Navigator.of(context).pop();
                      },
                      child: const SizedBox(
                        width: 125,
                        child: Text('View in Folder'),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Custom Button Widget
class Button extends StatelessWidget {
  final String text;
  final VoidCallback onPressed;
  final double? width;
  final double? height;
  final bool disabled;
  final Color? disabledColor;
  final bool noSplash;

  const Button(
      {super.key,
      required this.text,
      required this.onPressed,
      this.width,
      this.height,
      this.disabled = false,
      this.disabledColor,
      this.noSplash = false});

  @override
  Widget build(BuildContext context) {
    Widget child;

    if (width == null || height == null) {
      child = Text(text);
    } else {
      child = SizedBox(
        width: width!,
        height: height,
        child: Center(child: Text(text)),
      );
    }

    return ElevatedButton(
      onPressed: () {
        if (!disabled) {
          onPressed();
        }
      },
      style: ButtonStyle(
        splashFactory: noSplash ? NoSplash.splashFactory : null,
        backgroundColor: disabled
            ? WidgetStateProperty.all(disabledColor ?? Colors.grey)
            : WidgetStateProperty.all(Theme.of(context).colorScheme.primary),
        foregroundColor: WidgetStateProperty.all(Colors.white),
        overlayColor: disabled
            ? WidgetStateProperty.all(disabledColor ?? Colors.grey)
            : WidgetStateProperty.all(Theme.of(context).colorScheme.secondary),
        surfaceTintColor: WidgetStateProperty.all(Colors.transparent),
        mouseCursor: disabled
            ? WidgetStateProperty.all(SystemMouseCursors.basic)
            : WidgetStateProperty.all(SystemMouseCursors.click),
      ),
      child: child,
    );
  }
}

/// Custom TextInput Widget
class TextInput extends StatelessWidget {
  final String labelText;
  final String? hintText;
  final TextEditingController controller;
  final bool? obscureText;
  final bool? enabled;
  final void Function(String)? onChanged;
  final void Function(String)? onSubmitted;
  final Widget? error;

  const TextInput(
      {super.key,
      required this.labelText,
      this.hintText,
      required this.controller,
      this.obscureText,
      this.enabled,
      this.onChanged,
      this.onSubmitted,
      this.error});

  @override
  Widget build(BuildContext context) {
    return TextField(
      controller: controller,
      obscureText: obscureText ?? false,
      enabled: enabled,
      onChanged: onChanged,
      onSubmitted: onSubmitted,
      decoration: InputDecoration(
        labelText: labelText,
        hintText: hintText,
        hintStyle: const TextStyle(
            fontSize: 13,
            fontStyle: FontStyle.normal,
            color: Color(0xFFa9a9aa),
            fontWeight: FontWeight.w600),
        fillColor: Theme.of(context).colorScheme.tertiaryContainer,
        filled: true,
        error: error,
        border: const OutlineInputBorder(
          borderRadius: BorderRadius.all(Radius.circular(10.0)),
        ),
        contentPadding: const EdgeInsets.all(10.0),
      ),
    );
  }
}

/// Custom Switch widget
class CustomSwitch extends StatelessWidget {
  final bool value;
  final bool? disabled;
  final void Function(bool)? onChanged;

  const CustomSwitch(
      {super.key, required this.value, required this.onChanged, this.disabled});

  @override
  Widget build(BuildContext context) {
    return Transform.scale(
      scale: 0.85,
      child: Switch(
        value: value,
        onChanged: disabled == true ? null : onChanged,
        inactiveTrackColor: const Color(0xFF80848e),
        activeTrackColor: disabled == true
            ? const Color(0xFF80848e)
            : Theme.of(context).colorScheme.secondary,
      ),
    );
  }
}

/// A controller which helps bridge the gap between the UI and backend
class StateController extends ChangeNotifier {
  Contact? _activeContact;
  String status = 'Inactive';
  bool _deafened = false;
  bool _muted = false;
  bool inAudioTest = false;
  bool _callEndedRecently = false;
  bool callDisconnected = false;
  final Stopwatch _callTimer = Stopwatch();

  /// peerId, status
  final Map<String, String> sessions = {};

  /// active, restartable
  (bool, bool) _sessionManager = (false, false);

  DartNotify? _stopSendingScreenshare;
  DartNotify? _stopReceivingScreenshare;
  bool isSendingScreenshare = false;
  bool isReceivingScreenshare = false;

  Contact? get activeContact => _activeContact;
  bool get isCallActive => _activeContact != null;
  bool get isDeafened => _deafened;
  bool get isMuted => _muted;
  bool get callEndedRecently => _callEndedRecently;
  bool get blockAudioChanges => isCallActive || inAudioTest;
  bool get sessionManagerActive => _sessionManager.$1;
  bool get sessionManagerRestartable => _sessionManager.$2;
  String get callDuration =>
      formatElapsedTime(_callTimer.elapsed.inMilliseconds);

  void setActiveContact(Contact? contact) {
    _activeContact = contact;
    notifyListeners();
  }

  void setStatus(String status) {
    this.status = status;

    if (status == 'Inactive') {
      _activeContact = null;
      _callTimer.stop();
      _callTimer.reset();
      callDisconnected = false;
    } else if (status == 'Active') {
      _callTimer.start();
    }

    notifyListeners();
  }

  void setSessionManager(bool active, bool restartable) {
    _sessionManager = (active, restartable);
    notifyListeners();
  }

  bool isActiveContact(Contact contact) {
    return _activeContact?.id() == contact.id();
  }

  bool isOnlineContact(Contact contact) {
    String status = sessions[contact.peerId()] ?? 'Unknown';
    return status == 'Connected';
  }

  void updateSession(String peerId, String status) {
    sessions[peerId] = status;
    notifyListeners();
  }

  String sessionStatus(Contact contact) {
    return sessions[contact.peerId()] ?? 'Unknown';
  }

  void deafen() {
    _deafened = !_deafened;
    _muted = _deafened;
    notifyListeners();
  }

  void mute() {
    _muted = !_muted;
    notifyListeners();
  }

  void setInAudioTest() {
    inAudioTest = !inAudioTest;
    status = inAudioTest ? 'In Audio Test' : 'Inactive';

    notifyListeners();
  }

  void disableCallsTemporarily() {
    _callEndedRecently = true;

    Timer(const Duration(seconds: 1), () {
      _callEndedRecently = false;
    });
  }

  void screenshareStarted(DartNotify stop, bool sending) {
    if (sending) {
      DebugConsole.log('Sending screenshare started');
      _stopSendingScreenshare = stop;
      isSendingScreenshare = true;

      // this catches the sending screenshare being closed by the receiver
      Future.microtask(() async {
        await stop.notified();
        // if the screen share is still sending, stop the screenshare
        if (isSendingScreenshare) {
          stopScreenshare(true);
        }
      });
    } else {
      DebugConsole.log('Receiving screenshare started');
      _stopReceivingScreenshare = stop;
      isReceivingScreenshare = true;
    }

    notifyListeners();
  }

  void stopScreenshare(bool sending) {
    DebugConsole.log('Stopping screenshare sending: $sending');

    if (sending) {
      _stopSendingScreenshare?.notify();
      _stopSendingScreenshare = null;
      isSendingScreenshare = false;
    } else {
      _stopReceivingScreenshare?.notify();
      _stopReceivingScreenshare = null;
      isReceivingScreenshare = false;
    }

    notifyListeners();
  }

  /// a group of actions run when the call ends
  void endOfCall() {
    setActiveContact(null);
    setStatus('Inactive');
    disableCallsTemporarily();
    stopScreenshare(true);
    stopScreenshare(false);
  }
}

/// A controller responsible for managing the statistics of the call
class StatisticsController extends ChangeNotifier {
  Statistics? _statistics;

  int get latency => _statistics == null ? 0 : _statistics!.latency.toInt();
  double get inputLevel => _statistics == null ? 0 : _statistics!.inputLevel;
  double get outputLevel => _statistics == null ? 0 : _statistics!.outputLevel;
  String get upload => _statistics == null
      ? '?'
      : formatBandwidth(_statistics!.uploadBandwidth.toInt());
  String get download => _statistics == null
      ? '?'
      : formatBandwidth(_statistics!.downloadBandwidth.toInt());
  double get loss => _statistics == null ? 0 : _statistics!.loss;

  void setStatistics(Statistics statistics) {
    _statistics = statistics;
    notifyListeners();
  }
}

/// Removes the padding from a Slider
class CustomTrackShape extends RoundedRectSliderTrackShape {
  @override
  Rect getPreferredRect({
    required RenderBox parentBox,
    Offset offset = Offset.zero,
    required SliderThemeData sliderTheme,
    bool isEnabled = false,
    bool isDiscrete = false,
  }) {
    final trackHeight = sliderTheme.trackHeight;
    final trackLeft = offset.dx;
    final trackTop = offset.dy + (parentBox.size.height - trackHeight!) / 2;
    final trackWidth = parentBox.size.width;
    return Rect.fromLTWH(trackLeft, trackTop, trackWidth, trackHeight);
  }
}

/// Manages the state of chat messages and attachments
class ChatStateController extends ChangeNotifier {
  /// a list of messages in the chat
  List<ChatMessage> messages = [];

  /// a list of attachments to be sent with the next message
  List<(String, Uint8List)> attachments = [];

  /// a flag indicating if the chat is active and should be enabled
  bool active = false;

  /// the input field for the chat
  TextEditingController messageInput = TextEditingController();

  /// a list of files used in the chat which optionally display images
  Map<String, (File?, Image?)> files = {};

  final SoundPlayer soundPlayer;

  ChatStateController(this.soundPlayer);

  /// called when the call receives a message
  void messageReceived(ChatMessage message) async {
    messages.add(message);

    // handle any attachments
    for (var attachment in message.attachments()) {
      File? file = await saveFile(attachment.$2, attachment.$1);

      if (file == null) {
        continue;
      }

      // add the file record
      _addFile(attachment.$1, file, attachment.$2);
    }

    // remove attachment data from memory
    message.clearAttachments();
    notifyListeners();

    // TODO there is no message received sound asset
    // // play the received sound
    // soundPlayer.play(bytes: await readSeaBytes(''));
  }

  /// adds a file to the list of attachments
  void addAttachmentFile(String name, File file) async {
    final fileNameWithoutExtension = name.substring(0, name.lastIndexOf('.'));
    final fileExtension = name.substring(name.lastIndexOf('.'));
    String newName =
        '$fileNameWithoutExtension-${DateTime.now().millisecondsSinceEpoch}$fileExtension';

    Uint8List bytes = await file.readAsBytes();
    attachments.add((newName, bytes));
    _addFile(newName, file, bytes);
    notifyListeners();
  }

  /// adds an attachment from memory to the list of attachments
  void addAttachmentMemory(String name, Uint8List data) {
    final fileNameWithoutExtension = name.substring(0, name.lastIndexOf('.'));
    final fileExtension = name.substring(name.lastIndexOf('.'));
    String newName =
        '$fileNameWithoutExtension-${DateTime.now().millisecondsSinceEpoch}$fileExtension';

    attachments.add((newName, data));
    _addFile(newName, null, data);
    notifyListeners();
  }

  /// adds a file to the list of files, optionally displaying images
  void _addFile(String name, File? file, Uint8List data) {
    if (isValidImageFormat(name)) {
      Image? image = Image.memory(data, fit: BoxFit.contain);
      files[name] = (file, image);
    } else {
      files[name] = (file, null);
    }
  }

  /// clears the state of the chat
  void clearState() {
    messages.clear();
    attachments.clear();
    messageInput.clear();
    notifyListeners();
  }

  /// clears the input field and attachments
  void clearInput() {
    messageInput.clear();
    attachments.clear();
    notifyListeners();
  }

  bool isValidImageFormat(String fileName) {
    const validExtensions = ['jpg', 'jpeg', 'png', 'gif', 'bmp', 'webp'];
    final extension = fileName.split('.').last.toLowerCase();
    return validExtensions.contains(extension);
  }

  /// removes an attachment before being sent
  void removeAttachment(String name) {
    attachments.removeWhere((attachment) => attachment.$1 == name);
    files.remove(name);
    notifyListeners();
  }
}

/// Notifies listeners every second
class PeriodicNotifier extends ChangeNotifier {
  PeriodicNotifier() {
    Timer.periodic(const Duration(seconds: 1), (timer) {
      notifyListeners();
    });
  }
}

/// Shows an error modal
void showErrorDialog(BuildContext context, String title, String errorMessage) {
  showDialog(
    context: context,
    builder: (BuildContext context) {
      return AlertDialog(
        title: Text(title),
        content: Text(errorMessage),
        actions: <Widget>[
          TextButton(
            child: const Text('Close'),
            onPressed: () {
              Navigator.of(context).pop(); // Dismiss the dialog
            },
          ),
        ],
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(10),
        ),
      );
    },
  );
}

/// Prompts the user to accept an incoming call
Future<bool> acceptCallPrompt(BuildContext context, Contact contact) async {
  const timeout = Duration(seconds: 10);

  if (!context.mounted) {
    return false;
  }

  bool? result = await showDialog<bool>(
    context: context,
    barrierDismissible: false,
    builder: (BuildContext context) {
      Timer(timeout, () {
        if (context.mounted) {
          Navigator.of(context).pop(false);
        }
      });

      return AlertDialog(
        title: Text('Accept call from ${contact.nickname()}?'),
        actions: <Widget>[
          TextButton(
            child: const Text('Deny'),
            onPressed: () {
              Navigator.of(context).pop(false);
            },
          ),
          TextButton(
            child: const Text('Accept'),
            onPressed: () {
              Navigator.of(context).pop(true);
            },
          ),
        ],
      );
    },
  );

  return result ?? false;
}

/// Reads the bytes of a sea file from the assets
Future<List<int>> readSeaBytes(String assetName) {
  return readAssetBytes('sounds/$assetName.sea');
}

Future<void> updateDenoiseModel(String? model, Telepathy telepathy) async {
  if (model == null) {
    telepathy.setModel(model: null);
    return;
  }

  List<int> bytes = await readAssetBytes('models/$model.rnn');
  telepathy.setModel(model: Uint8List.fromList(bytes));
}

/// Reads the bytes of a file from the assets
Future<List<int>> readAssetBytes(String assetName) async {
  final ByteData data = await rootBundle.load('assets/$assetName');
  final List<int> bytes = data.buffer.asUint8List();
  return bytes;
}

/// Formats milliseconds into hours:minutes:seconds
String formatElapsedTime(int milliseconds) {
  int seconds = (milliseconds / 1000).truncate();
  int minutes = (seconds / 60).truncate();
  int hours = (minutes / 60).truncate();

  String hoursStr = (hours % 60).toString().padLeft(2, '0');
  String minutesStr = (minutes % 60).toString().padLeft(2, '0');
  String secondsStr = (seconds % 60).toString().padLeft(2, '0');

  return "$hoursStr:$minutesStr:$secondsStr";
}

String formatBandwidth(int bytes) {
  if (bytes < 100000) {
    return '${roundToTotalDigits(bytes / 1000)} KB';
  } else if (bytes < 100000000) {
    return '${roundToTotalDigits(bytes / 1000000)} MB';
  } else {
    return '${roundToTotalDigits(bytes / 1000000000)} GB';
  }
}

String roundToTotalDigits(double number) {
  // get the number of digits before the decimal point
  int integerDigits = number.abs().toInt().toString().length;

  // calculate the number of fractional digits needed
  int fractionalDigits = 3 - integerDigits;
  if (fractionalDigits < 0) {
    fractionalDigits =
        0; // ff the total digits is less than the integer part, we round to the integer part
  }

  // round to the required number of fractional digits
  return number.toStringAsFixed(fractionalDigits).padRight(4, '0');
}

// TODO verify cross-platform compatibility
Future<File?> saveFile(Uint8List fileBytes, String fileName) async {
  Directory? downloadsDirectory = await getDownloadsDirectory();

  if (downloadsDirectory != null) {
    final subdirectory = Directory('${downloadsDirectory.path}/Telepathy');
    if (!await subdirectory.exists()) {
      await subdirectory.create();
    }

    final file = File('${subdirectory.path}/$fileName');
    await file.writeAsBytes(fileBytes);

    return file;
  } else {
    DebugConsole.warn('Unable to get downloads directory');
    return null;
  }
}
