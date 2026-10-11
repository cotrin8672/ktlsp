package probe
fun <T> localPick(first:Class<out T>,second:Class<Invocation>):T=error("fixture")
@kotlin.jvm.JvmName("localPickOther")
fun localPick(second:Class<Other>,first:Class<Other>):Other=error("fixture")
fun use(router:NamedRouter,left:Class<Invocation>,right:Class<Other>,marker:Invocation,known:Other) {
 router.pick(marker=left,type=left).service()
 router.pick(type=right,marker=right).body()
 freePick(marker=left,type=left).service()
 freePick(marker=left,type=right).body()
 freePick(marker=right,value=right).body()
 localPick(second=left,first=left).service()
 localPick(second=left,first=right).body()
 localPick(first=right,second=right).body()
 probe.freePick(marker=left,type=left)
 probe.freePick(marker=left,type=right)
 probe.freePick(marker=right,value=right)
 known.body()
}
