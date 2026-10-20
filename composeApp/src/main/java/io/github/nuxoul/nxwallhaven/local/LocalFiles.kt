package io.github.nuxoul.nxwallhaven.local

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.core.content.FileProvider
import io.github.nuxoul.nxwallhaven.shared.Wall
import java.io.File
import java.io.InputStream
import java.io.OutputStream

/** Exact-byte platform export. The journal reconciles publication before a process crash. */
class LocalFiles(private val context:Context) {
    private val journal=context.getSharedPreferences("download-export-v1",Context.MODE_PRIVATE)
    private val resolver get()=context.contentResolver
    private fun privateRoot()=File(checkNotNull(context.getExternalFilesDir(android.os.Environment.DIRECTORY_PICTURES)){"图片存储不可用"},"NxWallhaven").apply{mkdirs()}.canonicalFile
    private fun file(uri:Uri):File {
        val original=File(requireNotNull(uri.path))
        require(!java.nio.file.Files.isSymbolicLink(original.toPath()))
        val path=original.canonicalFile
        require(path.parentFile==privateRoot()){"不是本应用图片目录"}
        return path
    }
    fun input(uri:Uri):InputStream = when(uri.scheme) {
        "content" -> {require(uri.authority==MediaStore.AUTHORITY);resolver.openInputStream(uri)?:error("图片不可用")}
        "file" -> file(uri).inputStream()
        else -> error("文件位置无效")
    }
    enum class Availability { AVAILABLE, MISSING, INACCESSIBLE }
    fun availability(value:String):Availability {
        return try {
            val uri=Uri.parse(value)
            val present=when(uri.scheme) {
                "content" -> {
                    require(uri.authority==MediaStore.AUTHORITY)
                    resolver.query(uri,arrayOf(MediaStore.Images.Media._ID),null,null,null)?.use{it.moveToFirst()}
                        ?:return Availability.INACCESSIBLE
                }
                "file" -> file(uri).exists()
                else -> return Availability.INACCESSIBLE
            }
            if(!present)Availability.MISSING else input(uri).use{if(it.read()>=0)Availability.AVAILABLE else Availability.INACCESSIBLE}
        }catch(_:Exception){Availability.INACCESSIBLE}
    }
    fun exists(uri:String)=availability(uri)==Availability.AVAILABLE
    fun shareUri(uri:String):Uri {
        val parsed=Uri.parse(uri)
        return if(parsed.scheme=="file")FileProvider.getUriForFile(context,"${context.packageName}.files",file(parsed))else parsed
    }
    fun delete(uri:String):Boolean {
        val parsed=Uri.parse(uri)
        return when(parsed.scheme){
            "content" -> {require(parsed.authority==MediaStore.AUTHORITY);resolver.delete(parsed,null,null)>0}
            "file" -> file(parsed).let{!it.exists() || it.delete()}
            else -> false
        }
    }
    /** commit serializes only final publication+index update against pause/cancel, not the file copy. */
    fun export(wall:Wall,source:File,keepGoing:()->Boolean,commit:(String,()->Unit)->Unit={_,publish->publish()}):String {
        require(wall.id.matches(Regex("[A-Za-z0-9]{6}")))
        check(keepGoing())
        if(Build.VERSION.SDK_INT<29)return exportPrivate(wall,source,keepGoing,commit)
        var uri=journal.getString(wall.id,null)?.let(Uri::parse)
        if(uri!=null) {
            require(uri.authority==MediaStore.AUTHORITY)
            val existing=resolver.query(uri,arrayOf(MediaStore.Images.Media.IS_PENDING,MediaStore.Images.Media.SIZE),null,null,null)?.use {
                if(it.moveToFirst())it.getInt(0) to it.getLong(1)else null
            }
            if(existing!=null && existing.first==0) {
                check(existing.second==source.length() && sameBytes(uri,source)){"已有导出文件内容不符，未覆盖"}
                commit(uri.toString()){}
                return uri.toString()
            }
            if(existing==null)uri=null
        }
        if(uri==null) {
            val values=ContentValues().apply{
                put(MediaStore.Images.Media.DISPLAY_NAME,wall.filename)
                put(MediaStore.Images.Media.MIME_TYPE,wall.format)
                put(MediaStore.Images.Media.RELATIVE_PATH,"Pictures/NxWallhaven")
                put(MediaStore.Images.Media.IS_PENDING,1)
            }
            uri=resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI,values)?:error("无法创建相册文件")
            if(!journal.edit().putString(wall.id,uri.toString()).commit()){
                resolver.delete(uri,null,null);error("无法记录导出状态")
            }
        }
        val target=uri
        var published=false
        try {
            resolver.openOutputStream(target,"wt")!!.use{out->source.inputStream().use{copy(it,out,keepGoing)}}
            commit(target.toString()) {
                check(keepGoing())
                check(resolver.update(target,ContentValues().apply{put(MediaStore.Images.Media.IS_PENDING,0)},null,null)>0)
                published=true
            }
            return target.toString()
        }catch(t:Throwable){
            // A committed user file survives an index failure; retry reconciles the retained journal.
            if(!published){runCatching{resolver.delete(target,null,null)};journal.edit().remove(wall.id).commit()}
            throw t
        }
    }
    private fun exportPrivate(wall:Wall,source:File,keepGoing:()->Boolean,commit:(String,()->Unit)->Unit):String {
        val target=File(privateRoot(),wall.filename)
        val uri=Uri.fromFile(target).toString()
        if(target.isFile && sameBytes(Uri.fromFile(target),source)){commit(uri){};return uri}
        check(!target.exists()){"同名文件已存在，未覆盖"}
        val part=File(privateRoot(),".${wall.filename}.part")
        try {
            source.inputStream().use{input->part.outputStream().use{copy(input,it,keepGoing)}}
            commit(uri){check(keepGoing());check(!target.exists());check(part.renameTo(target))}
            return uri
        }finally{part.delete()}
    }
    fun finish(id:String){check(journal.edit().remove(id).commit())}
    fun discardPending(id:String) {
        val uri=journal.getString(id,null)?.let(Uri::parse)?:return
        if(Build.VERSION.SDK_INT>=29 && uri.authority==MediaStore.AUTHORITY) {
            val pending=resolver.query(uri,arrayOf(MediaStore.Images.Media.IS_PENDING),null,null,null)?.use{it.moveToFirst() && it.getInt(0)==1}?:return
            // Keep a published URI for crash reconciliation; cancellation never deletes it.
            if(!pending)return
            resolver.delete(uri,null,null)
        }
        journal.edit().remove(id).commit()
    }
    private fun sameBytes(uri:Uri,source:File):Boolean=runCatching {
        fun hash(stream:InputStream):ByteArray=stream.use {
            val digest=java.security.MessageDigest.getInstance("SHA-256");val buffer=ByteArray(65536)
            while(true){val n=it.read(buffer);if(n<0)break;digest.update(buffer,0,n)}
            digest.digest()
        }
        hash(input(uri)).contentEquals(hash(source.inputStream()))
    }.getOrDefault(false)
    private fun copy(input:InputStream,out:OutputStream,keepGoing:()->Boolean) {
        val buffer=ByteArray(65536)
        while(true){check(keepGoing()){"下载已暂停或取消"};val n=input.read(buffer);if(n<0)break;out.write(buffer,0,n)}
        out.flush()
    }
}
