package app;

public class PlainUse {
    public int run(PlainOwner value) throws InterruptedException {
        value.accept("Ada");
        value.accept(1, 2);
        value.wait(1L);
        value.many(1, 2, 3);
        return value.identity(3);
    }
}
