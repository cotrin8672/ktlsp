package mapped

fun mappedControl(values: List<String>): MappedResult = MappedConsumer().consume(values)
