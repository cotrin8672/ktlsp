package hintcase;

class Box<T> {}
class Body implements AutoCloseable { public void close() {} }
class Other {}
class Factory {
    Body make(String value) { return new Body(); }
    Box<String> make(int value) { return new Box<String>(); }
    Box<? extends Body> producer() { return null; }
    Box<? super Body> consumer() { return null; }
    Box<?> unbounded() { return null; }
}
class Decoy {
    Other make(String value) { return new Other(); }
    Other supply() { return new Other(); }
    Other missingCall() { return new Other(); }
}

public class HintUse {
    private <T> T identity(T value) { return value; }

    public void run(Factory factory, Body body, KFactory producer) throws Exception {
        var longNumber = 1L;
        var floatNumber = 2.5F;
        var boxed = new Box<String>();
        var selected = factory.make("text");
        var overloaded = factory.make(0);
        var covariant = factory.producer();
        var contravariant = factory.consumer();
        var wildcard = factory.unbounded();
        var alias = selected;
        var generic = identity(body);
        var fromKotlin = producer.supply();
        try (var resource = body) {
            var resourceAlias = resource;
            resourceAlias.close();
        }
        Body explicit = body;
    }
}
