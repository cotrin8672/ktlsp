package binaryuse;

import arrayfixture.Factory;
import pluginfixture.Person;

public final class JavaUse {
    public void use(Factory factory, Person person) {
        person.getName();
        factory.rows();
        factory.counts();
        factory.annotated();
        factory.withArgument(1);
        Factory.staticRows();
    }
}
