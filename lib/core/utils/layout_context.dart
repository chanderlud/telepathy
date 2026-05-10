import 'package:flutter/widgets.dart';
import 'package:telepathy/core/constants/app_constants.dart';

extension LayoutContext on BuildContext {
  bool get isWideLayout =>
      MediaQuery.sizeOf(this).width > AppConstants.wideLayoutBreakpoint;

  bool get isCompactControls =>
      MediaQuery.sizeOf(this).height < AppConstants.compactHeightBreakpoint;

  bool get isCompactContacts =>
      isCompactControls && !isWideLayout;
}
