package probe.bounds

interface KotlinLeft {
    fun leftOnly(): Int
}

interface KotlinRight {
    fun rightOnly(): Int
}

interface KotlinReplacement {
    fun replacementOnly(): Int
}

fun <T> kotlinBounds(value: T): Int where T : KotlinLeft, T : KotlinRight {
    return value.leftOnly() + value.rightOnly()
}
