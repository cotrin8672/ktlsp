package probe
class NamedRouter {
 fun <T> pick(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
 @kotlin.jvm.JvmName("pickOther")
fun <T> pick(marker:Class<Other>,type:Class<out T>):T=error("fixture")
}
fun <T> freePick(type:Class<out T>,marker:Class<Invocation>):T=error("fixture")
@kotlin.jvm.JvmName("freePickOther")
fun freePick(marker:Class<Other>,value:Class<Other>):Other=error("fixture")
