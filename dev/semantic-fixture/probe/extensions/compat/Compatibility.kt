package compat

import vendor.Versioned

fun Versioned.compat(): Payload = Payload()
fun Versioned.reboundCompat(): Versioned = this
