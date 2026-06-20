package com.agentzero.client.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

private val LightColors = lightColorScheme(
    primary = Color(0xFF1A56DB),
    onPrimary = Color.White,
    secondary = Color(0xFF4F83FF),
    tertiary = Color(0xFF0D9488),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF4F83FF),
    onPrimary = Color(0xFF061230),
    secondary = Color(0xFF7EA5EB),
    tertiary = Color(0xFF2DD4BF),
)

@Composable
fun AgentZeroTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = if (isSystemInDarkTheme()) DarkColors else LightColors,
        content = content,
    )
}
