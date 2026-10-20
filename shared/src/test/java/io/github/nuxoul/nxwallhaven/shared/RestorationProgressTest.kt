package io.github.nuxoul.nxwallhaven.shared

import org.junit.Assert.*
import org.junit.Test

class RestorationProgressTest {
    @Test fun allFiltersRoundTripAndPermissionsAreReapplied() {
        val filters=Filters("@tester", "101", "111", "random", "asc", "3840x2160", "16x9", "#abcdef", "3M")
        assertEquals(filters, Filters.restore(filters.savedJson(),true,true))
        assertEquals("110", Filters.restore(filters.savedJson(),false,true).purity)
        assertEquals(Filters(), Filters.restore("{\"version\":1,\"apikey\":\"never\"}",true,true))
        assertEquals(Filters(), Filters.restore("{\"version\":99}",true,true))
    }
    @Test fun invalidSchemaIsNotSentToNetwork() {
        assertEquals(Filters(),Filters.restore(Filters(categories="00x").savedJson(),true,true))
        assertEquals(Filters(),Filters.restore(Filters(sorting="arbitrary").savedJson(),true,true))
        assertEquals("100",Filters(purity="bad").permitted(true,true).purity)
    }
    @Test fun nativeCountersNeverWrapAndUnknownTotalStaysUnknown() {
        val progress=TransferProgress.fromNative(ULong.MAX_VALUE, ULong.MAX_VALUE)
        assertEquals(Long.MAX_VALUE,progress.received)
        assertNull(progress.total)
        assertNull(progress.bytesPerSecond)
        assertNull(progress.etaSeconds)
        assertNull(TransferProgress.fromNative(5uL,null).fraction)
    }
    @Test fun rateNeedsElapsedTimeAndNewBytesNotCachedOrResumedBytes() {
        val meter=TransferMeter()
        assertNull(meter.sample(100uL,200uL,0L).bytesPerSecond)
        val progress=meter.sample(150uL,200uL,1_000_000_000L)
        assertEquals(50L,progress.bytesPerSecond)
        assertEquals(1L,progress.etaSeconds)
        assertNull(TransferMeter().sample(200uL,200uL,0L).bytesPerSecond)
        assertNull(meter.sample(10uL,null,2_000_000_000L).bytesPerSecond)
    }
}
