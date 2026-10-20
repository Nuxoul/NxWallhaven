package io.github.nuxoul.nxwallhaven

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

data class Prefs(val hasKey: Boolean=false, val allowNsfw: Boolean=false, val theme: Int=0, val grid: Boolean=false)

/** The preference file contains ciphertext/IV only; Android Keystore owns the AES key. */
class Preferences(context: Context) {
    private val store=context.getSharedPreferences("settings-v1",Context.MODE_PRIVATE)
    private val alias="NxWallhaven.ApiKey.v1"
    private fun secret(): SecretKey {
        val ks=KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(alias,null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES,"AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder(alias,KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    fun key(): String? {
        val encoded=store.getString("key-cipher",null)?:return null
        val iv=store.getString("key-iv",null)?:error("加密 Key 资料不完整，请删除后重新输入")
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE,secret(),GCMParameterSpec(128,Base64.decode(iv,Base64.NO_WRAP)))
            String(doFinal(Base64.decode(encoded,Base64.NO_WRAP)),Charsets.UTF_8)
        }
    }
    fun snapshot(): Prefs = Prefs(store.contains("key-cipher"),store.getBoolean("nsfw",false),store.getInt("theme",0),store.getBoolean("grid",false))
    fun save(input:String,allow:Boolean,theme:Int,grid:Boolean): Prefs {
        val edit=store.edit()
        if(input.isNotBlank()) {
            val cipher=Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE,secret()) }
            val bytes=cipher.doFinal(input.trim().toByteArray(Charsets.UTF_8))
            edit.putString("key-cipher",Base64.encodeToString(bytes,Base64.NO_WRAP))
                .putString("key-iv",Base64.encodeToString(cipher.iv,Base64.NO_WRAP))
        }
        val hasKey=input.isNotBlank() || store.contains("key-cipher")
        check(edit.putBoolean("nsfw",allow && hasKey).putInt("theme",theme.coerceIn(0,2)).putBoolean("grid",grid).commit()) { "设置保存失败，未应用" }
        return snapshot()
    }
    fun clearKey(): Prefs {
        check(store.edit().remove("key-cipher").remove("key-iv").putBoolean("nsfw",false).commit()) { "删除 Key 失败" }
        return snapshot()
    }
}
