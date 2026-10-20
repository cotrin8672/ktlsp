package defaultfixture

interface Other

private abstract class KotlinDefaults : Mid(), Other {
    fun use() = super.getBreakSpeed()
    fun generic(value: Overlay) = value.tooltip("ok")
}
