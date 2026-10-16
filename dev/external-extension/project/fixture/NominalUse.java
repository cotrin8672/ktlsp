package fixture; public class NominalUse { public boolean use(NominalTarget value, BlockPos pos) { return value.lookup(pos).solid(); } }
