package probe
fun <A,B> pick(first:Class<out A>,second:Class<out B>):A=error("fixture")
class NamedShapes {
 fun <T> optional(ignored:Other=Other(),required:Class<out T>):T=error("fixture")
}
interface LeftMarker
interface RightMarker
class BothMarker : LeftMarker, RightMarker
fun <T> ambiguous(type:Class<out T>,marker:LeftMarker):T=error("fixture")
@kotlin.jvm.JvmName("ambiguousOther")
fun <T> ambiguous(marker:RightMarker,type:Class<out T>):Other=error("fixture")
