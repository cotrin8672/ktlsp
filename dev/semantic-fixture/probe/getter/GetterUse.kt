package probe.getteruse
import probe.getter.PlainBean
fun use(bean: PlainBean) = bean.payload.payloadOnly()
