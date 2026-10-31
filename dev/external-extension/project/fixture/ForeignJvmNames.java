package fixture;

class ForeignJvmNames {
    static class Outer {
        static class Target {}
    }
    void foreignJvmProbe(ForeignJvmNames.Outer.Target target) {}
}
