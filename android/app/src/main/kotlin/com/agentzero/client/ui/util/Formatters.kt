package com.agentzero.client.ui.util

import java.text.SimpleDateFormat
import java.time.Instant
import java.util.Date
import java.util.Locale

fun formatIsoDateTime(value: String?): String {
    if (value.isNullOrBlank()) return "—"
    val formatter = SimpleDateFormat.getDateTimeInstance(
        SimpleDateFormat.SHORT,
        SimpleDateFormat.MEDIUM,
        Locale.getDefault(),
    )
    return runCatching {
        val instant = Instant.parse(value)
        formatter.format(Date.from(instant))
    }.getOrElse { value }
}
