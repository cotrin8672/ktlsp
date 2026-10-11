package probe.accounts

import kotlin.runCatching

data class Account(val email: String)

class Logger {
    fun error(t: Throwable, msg: () -> String) {}
}

val logger = Logger()

fun account(): Account = Account("demo@example.test")

fun probeAccountChain() {
    runCatching { account() }
        .onFailure { logger.error(it) { "failed" } }
        .getOrThrow()
        .ema
}

class SuperResult {
    fun publicOnly(): Int = 1
}

open class SuperBase {
    open fun pick(): SuperResult = SuperResult()
    private fun pick(vararg ignored: Int): String = "private"
}

interface SuperMarker

class SuperProbe : SuperBase(), SuperMarker {
    fun probe() = super.pick().publicO
}
