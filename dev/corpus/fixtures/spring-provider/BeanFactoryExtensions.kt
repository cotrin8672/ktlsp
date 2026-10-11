package oracle

inline fun <reified T : Any> BeanFactory.getBean(): T =
    getBeanProvider<T>().getObject()

inline fun <reified T : Any> BeanFactory.getBeanProvider(): ObjectProvider<T> =
    getBeanProvider(ResolvableType.forType((object : ParameterizedTypeReference<T>() {}).type))

inline fun <reified T : Any> BeanFactory.explicit(): T =
    this.getBeanProvider<T>().getObject()

fun typedProviderControl(provider: ObjectProvider<Payload>): String = provider.getObject().label()

class Payload {
    fun label(): String = "payload"
}
