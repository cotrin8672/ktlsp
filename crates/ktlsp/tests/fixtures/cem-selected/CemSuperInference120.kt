package ktlsp.cem.inference120
import com.simibubi.create.content.kinetics.drill.DrillBlockEntity
import io.github.cotrin8672.cem.content.block.EnchantableBlockEntity
import net.minecraft.core.BlockPos
import net.minecraft.world.level.block.entity.BlockEntityType
import net.minecraft.world.level.block.state.BlockState
private abstract class CemSuperInference120(type: BlockEntityType<*>, pos: BlockPos, state: BlockState)
    : DrillBlockEntity(type, pos, state), EnchantableBlockEntity {
    fun observe() {
        val speed = super.getBreakSpeed()
    }
}
