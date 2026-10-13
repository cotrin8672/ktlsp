package binaryuse

import arrayfixture.Factory
import pluginfixture.Person

fun useBinary(factory: Factory, person: Person) {
    person.getName()
    factory.rows()
    factory.counts()
    factory.annotated()
    factory.withArgument(1)
    Factory.staticRows()
}
