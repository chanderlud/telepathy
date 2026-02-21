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
    return Scaffold(
      body: Padding(
          padding: const EdgeInsets.all(20.0),
          child: SafeArea(
              bottom: false,
              child: LayoutBuilder(
                  builder: (BuildContext context, BoxConstraints constraints) {
                const Widget contactsList = SortedContactsList();

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
                                children: [
                                  // Animated call-details / form area
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

                                  // Contacts list always present, just expands when the left bit collapses
                                  Flexible(
                                    fit: FlexFit.loose,
                                    child: stateController.activeRoom != null
                                        ? const RoomDetailsWidget()
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
                    Container(
                      constraints: BoxConstraints(
                          maxHeight: 250, maxWidth: constraints.maxWidth),
                      child: contactsList,
                    ),
                    const SizedBox(height: 20),
                    HomeTabView(
                        widgetOne: const CallControls(),
                        widgetTwo: const Padding(
                            padding: EdgeInsets.only(
                                left: 10, right: 10, top: 5, bottom: 10),
                            child: ChatWidget()),
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
