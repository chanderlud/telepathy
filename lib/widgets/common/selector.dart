import 'package:flutter/material.dart';

/// A widget that listens to a [Listenable] and only rebuilds when a selected
/// value changes.
class Selector<T extends Listenable, S> extends StatefulWidget {
  final T listenable;
  final S Function(T) selector;
  final Widget Function(BuildContext, S) builder;
  final bool Function(S previous, S current)? shouldRebuild;

  const Selector({
    super.key,
    required this.listenable,
    required this.selector,
    required this.builder,
    this.shouldRebuild,
  });

  @override
  State<Selector<T, S>> createState() => _SelectorState<T, S>();
}

class _SelectorState<T extends Listenable, S> extends State<Selector<T, S>> {
  S? _previousValue;

  @override
  void initState() {
    super.initState();
    _previousValue = widget.selector(widget.listenable);
    widget.listenable.addListener(_onListenableChanged);
  }

  @override
  void didUpdateWidget(covariant Selector<T, S> oldWidget) {
    super.didUpdateWidget(oldWidget);

    if (oldWidget.listenable != widget.listenable) {
      oldWidget.listenable.removeListener(_onListenableChanged);
      widget.listenable.addListener(_onListenableChanged);
    }

    if (oldWidget.selector != widget.selector ||
        oldWidget.listenable != widget.listenable) {
      final nextValue = widget.selector(widget.listenable);
      setState(() {
        _previousValue = nextValue;
      });
    }
  }

  void _onListenableChanged() {
    if (!mounted) return;

    final nextValue = widget.selector(widget.listenable);
    final previousValue = _previousValue as S;

    final bool rebuild = widget.shouldRebuild != null
        ? widget.shouldRebuild!(previousValue, nextValue)
        : previousValue != nextValue;

    if (!rebuild) return;

    setState(() {
      _previousValue = nextValue;
    });
  }

  @override
  void dispose() {
    widget.listenable.removeListener(_onListenableChanged);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return widget.builder(context, _previousValue as S);
  }
}


