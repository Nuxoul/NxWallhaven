package io.github.nuxoul.nxwallhaven.shared

import org.junit.Assert.*
import org.junit.Test

class PaginationTest {
    @Test fun appendDeduplicatesAndOnePageIsInFlight() {
        val pages=PageAccumulator();val gen=pages.reset()
        assertEquals(gen to 1,pages.begin());assertNull(pages.begin())
        assertTrue(pages.accept(gen,Page(listOf(Wall("a"),Wall("a"),Wall("b")),1,4,"seed")))
        assertEquals(listOf("a","b"),pages.walls.map {it.id})
        assertEquals(gen to 2,pages.begin())
        pages.accept(gen,Page(listOf(Wall("b"),Wall("c")),2,4,null))
        assertEquals("seed",pages.seed);assertEquals(3,pages.walls.size)
        pages.begin();pages.accept(gen,Page(listOf(Wall("c")),3,4,null))
        assertTrue(pages.exhausted);assertNull(pages.begin())
    }
    @Test fun refreshRejectsOldResultsAndFailuresCanRetry() {
        val pages=PageAccumulator();val old=pages.reset();pages.begin();val current=pages.reset();pages.begin()
        assertFalse(pages.accept(old,Page(listOf(Wall("old")),1,2,null)))
        assertTrue(pages.loading);pages.fail(old);assertTrue(pages.loading)
        pages.fail(current);assertEquals(current to 1,pages.begin())
        pages.accept(current,Page(emptyList(),1,1,null));assertTrue(pages.exhausted)
    }
    @Test fun purityRequiresPermissionAndKeyAndFallsBackSafe() {
        assertEquals("100",Filters(purity="001").permitted(false,true).purity)
        assertEquals("110",Filters(purity="111").permitted(true,false).purity)
        assertEquals("001",Filters(purity="001").permitted(true,true).purity)
    }
    @Test fun wireKeepsMissingMetadataAndUncroppedThumb() {
        val p=Wire.page("""{"data":[{"id":"a","thumbs":{"original":"uncropped","large":"cropped"}}],"meta":{"current_page":"1","last_page":"2"}}""")
        assertEquals("uncropped",p.walls.single().thumbnail)
        assertNull(p.walls.single().views);assertEquals("—",p.walls.single().resolution)
        assertEquals(2,p.last)
    }
}
