package fixture
import org.spongepowered.asm.mixin.Mixin
import org.spongepowered.asm.mixin.Shadow
@Mixin(NominalTarget::class)
abstract class NominalKotlinMixin {
    @field:Shadow private lateinit var anchor: BlockPos
    @Shadow abstract fun lookup(pos: BlockPos): BlockState
    @Shadow abstract fun positions(state: BlockState): Array<BlockPos>
    fun use(pos: BlockPos): Boolean = lookup(pos).solid()
}
