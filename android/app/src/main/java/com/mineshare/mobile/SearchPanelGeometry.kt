package com.mineshare.mobile

data class SearchPanelGeometry(val width: Int, val height: Int, val top: Int, val compact: Boolean, val horizontalOffset: Int)

fun searchPanelGeometry(width: Int, height: Int, density: Float, topInset: Int, bottomInset: Int,
    leftInset: Int, rightInset: Int, keyboardInset: Int): SearchPanelGeometry {
    fun dp(value: Int) = (value * density).toInt()
    val availableWidth = (width - leftInset - rightInset - dp(24)).coerceAtLeast(1)
    val availableHeight = (height - topInset - maxOf(bottomInset, keyboardInset) - dp(24)).coerceAtLeast(1)
    val panelHeight = minOf(dp(420), availableHeight)
    return SearchPanelGeometry(minOf(dp(520), availableWidth), panelHeight, topInset + dp(12), panelHeight < dp(240),
        (leftInset - rightInset) / 2)
}
