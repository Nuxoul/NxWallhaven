package io.github.nuxoul.nxwallhaven.shared

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class DownloadModelsTest {
    @Test fun apiShapeRoundTripsOriginalMetadata() {
        val wall=Wall(id="abc123",original="https://w.wallhaven.cc/full/ab/wallhaven-abc123.png",thumbnail="https://th.wallhaven.cc/orig/ab/abc123.jpg",width=12000,height=8000,resolution="12000x8000",purity="sfw",size=90_000_000,format="image/png",uploader="fixture",tags=listOf(7 to "landscape"),colors=listOf("#123456"))
        val parsed=DownloadWire.item(JSONObject().put("id",wall.id).put("wall",DownloadWire.wall(wall)).put("state","queued"))
        assertEquals(wall.original,parsed.wall.original)
        assertEquals(wall.thumbnail,parsed.wall.thumbnail)
        assertEquals(12000,parsed.wall.width)
        assertEquals(wall.tags,parsed.wall.tags)
        assertEquals(wall.uploader,parsed.wall.uploader)
        assertNull(parsed.total);assertNull(parsed.uri)
        assertTrue(parsed.active)
    }
    @Test fun unknownLengthIsNotFakeProgressAndPercentIsBounded() {
        val item=DownloadItem("abc123",Wall("abc123"),"running",received=12)
        assertNull(item.percent)
        assertNull(item.copy(total=0).percent)
        assertEquals(1f,item.copy(total=10).percent!!,0f)
        assertFalse(item.copy(state="paused").active)
        assertTrue(item.copy(state="completed").complete)
    }
}
