import 'package:flutter/material.dart' hide Overlay;
import 'package:provider/provider.dart';
import 'package:telepathy/controllers/index.dart';
import 'package:telepathy/widgets/call/call.dart';
import 'package:telepathy/widgets/chat/chat.dart';
import 'package:telepathy/widgets/contacts/contacts.dart';
import 'package:telepathy/widgets/home/home_tab_view.dart';

/// The main body of the app.
class HomePage extends StatelessWidget {
  const HomePage({super.key});

  @override
  Widget build(BuildContext context) {
    const Widget contactsList = SortedContactsList();

    return Scaffold(
      body: Padding(
          padding: const EdgeInsets.all(20.0),
          child: SafeArea(
              bottom: true,
              child: LayoutBuilder(
                  builder: (BuildContext context, BoxConstraints constraints) {
                if (constraints.maxWidth > 600) {
                  return Column(
                    children: [
                      Consumer<StateController>(
                        builder: (BuildContext context,
                            StateController stateController, Widget? child) {
                          return Container(
                              constraints: const BoxConstraints(maxHeight: 275),
                              child: Row(
                                mainAxisSize: MainAxisSize.min,
                                crossAxisAlignment: CrossAxisAlignment.stretch,
                                children: [
                                  AnimatedSize(
                                    duration: const Duration(milliseconds: 250),
                                    curve: Curves.easeInOut,
                                    alignment: Alignment.centerLeft,
                                    child: stateController.isCallActive
                                        ? Row(
                                            mainAxisSize: MainAxisSize.min,
                                            children: [
                                              Container(
                                                constraints:
                                                    const BoxConstraints(
                                                        maxWidth: 300),
                                                child:
                                                    const CallDetailsWidget(),
                                              ),
                                              const SizedBox(width: 20),
                                            ],
                                          )
                                        : const SizedBox.shrink(),
                                  ),
                                  Expanded(
                                    child: stateController.activeRoom != null
                                        ? Row(
                                            crossAxisAlignment:
                                                CrossAxisAlignment.stretch,
                                            children: [
                                              Flexible(
                                                flex: 5,
                                                child: RoomDetailsWidget(),
                                              ),
                                              const SizedBox(width: 20),
                                              const Expanded(
                                                flex: 6,
                                                child: contactsList,
                                              ),
                                            ],
                                          )
                                        : contactsList,
                                  ),
                                ],
                              ));
                        },
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
                                child: const CallControls()),
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
                                    child: const ChatWidget()))
                          ])),
                    ],
                  );
                } else {
                  return Column(children: [
                    Consumer<StateController>(
                      builder: (BuildContext context,
                          StateController stateController, Widget? child) {
                        return Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            if (stateController.activeRoom != null) ...[
                              const SizedBox(
                                height: 200,
                                child: RoomDetailsWidget(),
                              ),
                              const SizedBox(height: 12),
                            ],
                            contactsList,
                          ],
                        );
                      },
                    ),
                    const SizedBox(height: 20),
                    Consumer<StateController>(
                      builder: (BuildContext context,
                          StateController stateController, Widget? child) {
                      final scheme = Theme.of(context).colorScheme;
                      return HomeTabView(
                        key: ValueKey(
                            'home_tabs_${stateController.isCallActive}'),
                        widgetOne: const CallControls(),
                        widgetTwo: const Padding(
                            padding: EdgeInsets.only(
                                left: 10, right: 10, top: 5, bottom: 10),
                            child: ChatWidget()),
                        colorOne: scheme.tertiaryContainer,
                        colorTwo: scheme.secondaryContainer,
                        iconOne: const Icon(Icons.call),
                        iconTwo: const Icon(Icons.chat),
                        widgetThree: stateController.isCallActive
                            ? const Padding(
                                padding: EdgeInsets.symmetric(
                                    horizontal: 12, vertical: 8),
                                child: CallDetailsWidget(),
                              )
                            : null,
                        colorThree: scheme.secondaryContainer,
                        iconThree: stateController.isCallActive
                            ? const Icon(Icons.analytics_outlined)
                            : null,
                      );
                    }),
                  ]);
                }
              }))),
    );
  }
}
