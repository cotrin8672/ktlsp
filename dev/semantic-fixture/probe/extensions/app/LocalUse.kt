package app

import vendor.Versioned
import compat.Payload

fun localUse(v: Versioned): Int {
    fun Versioned.oldApi(): Payload = Payload() // local declaration
    return v.oldApi().payloadOnly() // local call
}

fun siblingUse(v: Versioned): Int {
    fun Versioned.oldApi(): Versioned = this // sibling declaration
    return v.oldApi().newApi() // sibling call
}
