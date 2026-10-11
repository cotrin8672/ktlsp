package app

import vendor.Versioned
import compat.compat as oldApi

fun aliasChain(v: Versioned): Int = v.oldApi().payloadOnly()
fun memberControl(v: Versioned): Int = v.newApi()
