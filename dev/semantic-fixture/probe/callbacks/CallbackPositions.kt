package probe.callbacks

class CallbackCookie {
    fun cookieOnlyCallback(): String = "cookie"
}

class CallbackBatch<T>(val value: T) {
    fun forEachIndexed(block: (Int, T) -> String): String = block(0, value)
}

fun Int.indexOnlyCallback(): String = "index"

fun inferredIndex(batch: CallbackBatch<CallbackCookie>): String =
    batch.forEachIndexed { index, cookie -> index.indexOnlyCallback() }

fun inferredCookie(batch: CallbackBatch<CallbackCookie>): String =
    batch.forEachIndexed { index, cookie -> cookie.cookieOnlyCallback() }

fun typedIndex(batch: CallbackBatch<CallbackCookie>): String =
    batch.forEachIndexed { typedIndex: Int, typedCookie: CallbackCookie -> typedIndex.indexOnlyCallback() }

fun typedCookie(batch: CallbackBatch<CallbackCookie>): String =
    batch.forEachIndexed { typedIndex: Int, typedCookie: CallbackCookie -> typedCookie.cookieOnlyCallback() }
