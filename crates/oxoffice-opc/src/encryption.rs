use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use aes::{Aes128, Aes192, Aes256};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::NoPadding};
use hmac::{Hmac, KeyInit, Mac};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::reader::NsReader;
use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha384, Sha512};

use crate::error::{OpcError, Result};
use crate::package::PackageReadLimits;

const AGILE_MAJOR_VERSION: u16 = 4;
const AGILE_MINOR_VERSION: u16 = 4;
const AGILE_RESERVED: u32 = 0x40;
const ENCRYPTION_NS: &[u8] = b"http://schemas.microsoft.com/office/2006/encryption";
const PASSWORD_NS: &[u8] = b"http://schemas.microsoft.com/office/2006/keyEncryptor/password";
const PASSWORD_URI: &str = "http://schemas.microsoft.com/office/2006/keyEncryptor/password";
const MAX_ENCRYPTION_INFO_BYTES: u64 = 1_048_576;
const MAX_SALT_BYTES: usize = 65_536;
const MAX_SPIN_COUNT: u32 = 10_000_000;
const PACKAGE_SEGMENT_BYTES: usize = 4_096;
const WRITE_KEY_BITS: u16 = 256;
const WRITE_SALT_BYTES: usize = 16;
const WRITE_SPIN_COUNT: u32 = 100_000;
const WRITE_HASH: HashAlgorithm = HashAlgorithm::Sha512;
const DATA_SPACES_STORAGE: &str = "/\u{6}DataSpaces";
const STRONG_ENCRYPTION_DATA_SPACE: &str = "StrongEncryptionDataSpace";
const STRONG_ENCRYPTION_TRANSFORM: &str = "StrongEncryptionTransform";

const VERIFIER_INPUT_BLOCK_KEY: [u8; 8] = [0xfe, 0xa7, 0xd2, 0x76, 0x3b, 0x4b, 0x9e, 0x79];
const VERIFIER_HASH_BLOCK_KEY: [u8; 8] = [0xd7, 0xaa, 0x0f, 0x6d, 0x30, 0x61, 0x34, 0x4e];
const PACKAGE_KEY_BLOCK_KEY: [u8; 8] = [0x14, 0x6e, 0x0b, 0xe7, 0xab, 0xac, 0xd0, 0xd6];
const HMAC_KEY_BLOCK_KEY: [u8; 8] = [0x5f, 0xb2, 0xad, 0x01, 0x0c, 0xb9, 0xe1, 0xf6];
const HMAC_VALUE_BLOCK_KEY: [u8; 8] = [0xa0, 0x67, 0x7f, 0x02, 0xb2, 0x2c, 0x84, 0x33];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashAlgorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

enum HmacState {
    Sha1(Hmac<Sha1>),
    Sha256(Hmac<Sha256>),
    Sha384(Hmac<Sha384>),
    Sha512(Hmac<Sha512>),
}

impl HmacState {
    fn new(algorithm: HashAlgorithm, key: &[u8]) -> Result<Self> {
        macro_rules! initialize {
            ($digest:ty, $variant:ident) => {
                <Hmac<$digest> as KeyInit>::new_from_slice(key)
                    .map(Self::$variant)
                    .map_err(|_| OpcError::InvalidEncryptionInfo)
            };
        }
        match algorithm {
            HashAlgorithm::Sha1 => initialize!(Sha1, Sha1),
            HashAlgorithm::Sha256 => initialize!(Sha256, Sha256),
            HashAlgorithm::Sha384 => initialize!(Sha384, Sha384),
            HashAlgorithm::Sha512 => initialize!(Sha512, Sha512),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            Self::Sha1(mac) => mac.update(data),
            Self::Sha256(mac) => mac.update(data),
            Self::Sha384(mac) => mac.update(data),
            Self::Sha512(mac) => mac.update(data),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            Self::Sha1(mac) => mac.finalize().into_bytes().to_vec(),
            Self::Sha256(mac) => mac.finalize().into_bytes().to_vec(),
            Self::Sha384(mac) => mac.finalize().into_bytes().to_vec(),
            Self::Sha512(mac) => mac.finalize().into_bytes().to_vec(),
        }
    }
}

impl HashAlgorithm {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "SHA1" | "SHA-1" => Ok(Self::Sha1),
            "SHA256" | "SHA-256" => Ok(Self::Sha256),
            "SHA384" | "SHA-384" => Ok(Self::Sha384),
            "SHA512" | "SHA-512" => Ok(Self::Sha512),
            _ => Err(OpcError::UnsupportedEncryptionAlgorithm(value.to_owned())),
        }
    }

    fn output_size(self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
            Self::Sha384 => 48,
            Self::Sha512 => 64,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Sha1 => "SHA1",
            Self::Sha256 => "SHA256",
            Self::Sha384 => "SHA384",
            Self::Sha512 => "SHA512",
        }
    }

    fn digest(self, data: &[u8]) -> Vec<u8> {
        match self {
            Self::Sha1 => Sha1::digest(data).to_vec(),
            Self::Sha256 => Sha256::digest(data).to_vec(),
            Self::Sha384 => Sha384::digest(data).to_vec(),
            Self::Sha512 => Sha512::digest(data).to_vec(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CipherParameters {
    salt: Vec<u8>,
    key_bits: u16,
    hash: HashAlgorithm,
}

impl CipherParameters {
    fn from_element(element: &BytesStart<'_>) -> Result<Self> {
        let salt_size = parse_usize_attribute(element, b"saltSize")?;
        if salt_size == 0 || salt_size > MAX_SALT_BYTES {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        if parse_usize_attribute(element, b"blockSize")? != 16 {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        let key_bits = parse_u16_attribute(element, b"keyBits")?;
        if !matches!(key_bits, 128 | 192 | 256) {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        if required_attribute(element, b"cipherAlgorithm")? != "AES"
            || required_attribute(element, b"cipherChaining")? != "ChainingModeCBC"
        {
            return Err(OpcError::UnsupportedEncryptionAlgorithm(
                "agile encryption requires AES-CBC".to_owned(),
            ));
        }
        let hash = HashAlgorithm::parse(&required_attribute(element, b"hashAlgorithm")?)?;
        if parse_usize_attribute(element, b"hashSize")? != hash.output_size() {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        let salt = decode_attribute(element, b"saltValue")?;
        if salt.len() != salt_size {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        Ok(Self {
            salt,
            key_bits,
            hash,
        })
    }

    fn key_bytes(&self) -> usize {
        usize::from(self.key_bits / 8)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PasswordKeyEncryptor {
    parameters: CipherParameters,
    spin_count: u32,
    encrypted_verifier_input: Vec<u8>,
    encrypted_verifier_hash: Vec<u8>,
    encrypted_package_key: Vec<u8>,
}

impl PasswordKeyEncryptor {
    fn from_element(element: &BytesStart<'_>) -> Result<Self> {
        let parameters = CipherParameters::from_element(element)?;
        let spin_count = parse_u32_attribute(element, b"spinCount")?;
        if spin_count > MAX_SPIN_COUNT {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        let encrypted_verifier_input = decode_attribute(element, b"encryptedVerifierHashInput")?;
        let encrypted_verifier_hash = decode_attribute(element, b"encryptedVerifierHashValue")?;
        let encrypted_package_key = decode_attribute(element, b"encryptedKeyValue")?;
        validate_encrypted_field(
            &encrypted_verifier_input,
            round_up(parameters.salt.len(), 16)?,
        )?;
        validate_encrypted_field(
            &encrypted_verifier_hash,
            round_up(parameters.hash.output_size(), 16)?,
        )?;
        Ok(Self {
            parameters,
            spin_count,
            encrypted_verifier_input,
            encrypted_verifier_hash,
            encrypted_package_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DataIntegrity {
    encrypted_hmac_key: Vec<u8>,
    encrypted_hmac_value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EncryptionDescriptor {
    key_data: CipherParameters,
    integrity: DataIntegrity,
    password: PasswordKeyEncryptor,
}

impl EncryptionDescriptor {
    fn parse(stream: &[u8]) -> Result<Self> {
        if stream.len() < 8 {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        let major = u16::from_le_bytes([stream[0], stream[1]]);
        let minor = u16::from_le_bytes([stream[2], stream[3]]);
        let reserved = u32::from_le_bytes([stream[4], stream[5], stream[6], stream[7]]);
        if major != AGILE_MAJOR_VERSION
            || minor != AGILE_MINOR_VERSION
            || reserved != AGILE_RESERVED
        {
            return Err(OpcError::UnsupportedEncryption(
                "only ECMA-376 agile encryption version 4.4 is supported",
            ));
        }
        let xml = &stream[8..];
        let xml = &xml[..xml
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(0, |index| index + 1)];
        parse_descriptor_xml(xml)
    }

    fn authenticate_password(&self, password: &str) -> Result<Vec<u8>> {
        if password.chars().count() > 255 {
            return Err(OpcError::InvalidPassword);
        }
        let base_hash = password_hash(password, &self.password)?;
        let verifier_key = derived_password_key(
            &base_hash,
            &VERIFIER_INPUT_BLOCK_KEY,
            &self.password.parameters,
        );
        let verifier_input = decrypt_aes_cbc(
            &self.password.encrypted_verifier_input,
            &verifier_key,
            &self.password.parameters.salt,
        )?;
        let verifier_hash_key = derived_password_key(
            &base_hash,
            &VERIFIER_HASH_BLOCK_KEY,
            &self.password.parameters,
        );
        let verifier_hash = decrypt_aes_cbc(
            &self.password.encrypted_verifier_hash,
            &verifier_hash_key,
            &self.password.parameters.salt,
        )?;
        let verifier_input = verifier_input
            .get(..self.password.parameters.salt.len())
            .ok_or(OpcError::InvalidEncryptionInfo)?;
        let expected = self.password.parameters.hash.digest(verifier_input);
        if !constant_time_eq(&verifier_hash[..expected.len()], &expected) {
            return Err(OpcError::InvalidPassword);
        }
        let package_key_key = derived_password_key(
            &base_hash,
            &PACKAGE_KEY_BLOCK_KEY,
            &self.password.parameters,
        );
        let package_key = decrypt_aes_cbc(
            &self.password.encrypted_package_key,
            &package_key_key,
            &self.password.parameters.salt,
        )?;
        Ok(package_key[..self.key_data.key_bytes()].to_vec())
    }

    fn authenticate_stream<R: Read>(&self, package_key: &[u8], mut encrypted: R) -> Result<()> {
        let hmac_key_iv = initialization_vector(
            &self.key_data.salt,
            Some(&HMAC_KEY_BLOCK_KEY),
            self.key_data.hash,
        );
        let hmac_key = decrypt_aes_cbc(
            &self.integrity.encrypted_hmac_key,
            package_key,
            &hmac_key_iv,
        )?;
        let hmac_value_iv = initialization_vector(
            &self.key_data.salt,
            Some(&HMAC_VALUE_BLOCK_KEY),
            self.key_data.hash,
        );
        let hmac_value = decrypt_aes_cbc(
            &self.integrity.encrypted_hmac_value,
            package_key,
            &hmac_value_iv,
        )?;
        let hmac_key_len =
            if self.integrity.encrypted_hmac_key.len() == round_up(self.key_data.salt.len(), 16)? {
                self.key_data.salt.len()
            } else {
                self.key_data.hash.output_size()
            };
        if hmac_key.len() < hmac_key_len || hmac_value.len() < self.key_data.hash.output_size() {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        let mut mac = HmacState::new(self.key_data.hash, &hmac_key[..hmac_key_len])?;
        let mut buffer = [0_u8; 8_192];
        loop {
            let read = encrypted.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            mac.update(&buffer[..read]);
        }
        let expected = mac.finalize();
        if !constant_time_eq(&hmac_value[..self.key_data.hash.output_size()], &expected) {
            return Err(OpcError::EncryptedPackageIntegrity);
        }
        Ok(())
    }

    fn to_stream(&self) -> Result<Vec<u8>> {
        let xml = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n",
                r#"<encryption xmlns="http://schemas.microsoft.com/office/2006/encryption" xmlns:p="http://schemas.microsoft.com/office/2006/keyEncryptor/password" xmlns:c="http://schemas.microsoft.com/office/2006/keyEncryptor/certificate">"#,
                r#"<keyData saltSize="{key_salt_size}" blockSize="16" keyBits="{key_bits}" hashSize="{hash_size}" cipherAlgorithm="AES" cipherChaining="ChainingModeCBC" hashAlgorithm="{key_hash}" saltValue="{key_salt}"/>"#,
                r#"<dataIntegrity encryptedHmacKey="{hmac_key}" encryptedHmacValue="{hmac_value}"/>"#,
                r#"<keyEncryptors><keyEncryptor uri="{password_uri}">"#,
                r#"<p:encryptedKey spinCount="{spin_count}" saltSize="{password_salt_size}" blockSize="16" keyBits="{password_bits}" hashSize="{password_hash_size}" cipherAlgorithm="AES" cipherChaining="ChainingModeCBC" hashAlgorithm="{password_hash}" saltValue="{password_salt}" encryptedVerifierHashInput="{verifier_input}" encryptedVerifierHashValue="{verifier_hash}" encryptedKeyValue="{package_key}"/>"#,
                r#"</keyEncryptor></keyEncryptors></encryption>"#,
            ),
            key_salt_size = self.key_data.salt.len(),
            key_bits = self.key_data.key_bits,
            hash_size = self.key_data.hash.output_size(),
            key_hash = self.key_data.hash.name(),
            key_salt = BASE64_STANDARD.encode(&self.key_data.salt),
            hmac_key = BASE64_STANDARD.encode(&self.integrity.encrypted_hmac_key),
            hmac_value = BASE64_STANDARD.encode(&self.integrity.encrypted_hmac_value),
            password_uri = PASSWORD_URI,
            spin_count = self.password.spin_count,
            password_salt_size = self.password.parameters.salt.len(),
            password_bits = self.password.parameters.key_bits,
            password_hash_size = self.password.parameters.hash.output_size(),
            password_hash = self.password.parameters.hash.name(),
            password_salt = BASE64_STANDARD.encode(&self.password.parameters.salt),
            verifier_input = BASE64_STANDARD.encode(&self.password.encrypted_verifier_input),
            verifier_hash = BASE64_STANDARD.encode(&self.password.encrypted_verifier_hash),
            package_key = BASE64_STANDARD.encode(&self.password.encrypted_package_key),
        );
        let mut stream = Vec::with_capacity(8 + xml.len());
        stream.extend_from_slice(&AGILE_MAJOR_VERSION.to_le_bytes());
        stream.extend_from_slice(&AGILE_MINOR_VERSION.to_le_bytes());
        stream.extend_from_slice(&AGILE_RESERVED.to_le_bytes());
        stream.extend_from_slice(xml.as_bytes());
        if stream.len() as u64 > MAX_ENCRYPTION_INFO_BYTES {
            return Err(OpcError::InvalidEncryptionInfo);
        }
        Ok(stream)
    }
}

pub(crate) fn write_encrypted_package(
    output: &mut Vec<u8>,
    plaintext_zip: &[u8],
    password: &str,
) -> Result<()> {
    let fill_random = |output: &mut [u8]| {
        getrandom::fill(output).map_err(|error| {
            OpcError::Io(std::io::Error::other(format!(
                "operating system random source failed: {error}"
            )))
        })
    };
    let staged = encrypted_package_bytes_with_random(plaintext_zip, password, fill_random)?;
    append_staged_with_reserve(output, &staged, |output, additional| {
        output.try_reserve(additional).map_err(|error| {
            OpcError::Io(std::io::Error::other(format!(
                "failed to reserve encrypted output: {error}"
            )))
        })
    })
}

fn append_staged_with_reserve(
    output: &mut Vec<u8>,
    staged: &[u8],
    reserve: impl FnOnce(&mut Vec<u8>, usize) -> Result<()>,
) -> Result<()> {
    let original_len = output.len();
    if let Err(error) = reserve(output, staged.len()) {
        output.truncate(original_len);
        return Err(error);
    }
    if output.len() != original_len {
        output.truncate(original_len);
        return Err(OpcError::InvalidEncryptedPackage);
    }
    output.extend_from_slice(staged);
    Ok(())
}

fn encrypted_package_bytes_with_random(
    plaintext_zip: &[u8],
    password: &str,
    mut fill_random: impl FnMut(&mut [u8]) -> Result<()>,
) -> Result<Vec<u8>> {
    if password.is_empty() || password.chars().count() > 255 {
        return Err(OpcError::InvalidPassword);
    }
    let plaintext_len =
        u64::try_from(plaintext_zip.len()).map_err(|_| OpcError::InvalidEncryptedPackage)?;
    let expected_ciphertext = encrypted_package_ciphertext_len(plaintext_zip.len())?;
    let _ = expected_ciphertext
        .checked_add(8)
        .ok_or(OpcError::InvalidEncryptedPackage)?;

    let mut key_data_salt = [0_u8; WRITE_SALT_BYTES];
    let mut password_salt = [0_u8; WRITE_SALT_BYTES];
    let mut package_key = [0_u8; 32];
    let mut verifier = [0_u8; WRITE_SALT_BYTES];
    let mut hmac_key = [0_u8; 64];
    fill_random(&mut key_data_salt)?;
    fill_random(&mut password_salt)?;
    fill_random(&mut package_key)?;
    fill_random(&mut verifier)?;
    fill_random(&mut hmac_key)?;

    let key_data = CipherParameters {
        salt: key_data_salt.to_vec(),
        key_bits: WRITE_KEY_BITS,
        hash: WRITE_HASH,
    };
    let password_parameters = CipherParameters {
        salt: password_salt.to_vec(),
        key_bits: WRITE_KEY_BITS,
        hash: WRITE_HASH,
    };
    let mut password_encryptor = PasswordKeyEncryptor {
        parameters: password_parameters.clone(),
        spin_count: WRITE_SPIN_COUNT,
        encrypted_verifier_input: Vec::new(),
        encrypted_verifier_hash: Vec::new(),
        encrypted_package_key: Vec::new(),
    };
    let base_hash = password_hash(password, &password_encryptor)?;
    let verifier_key =
        derived_password_key(&base_hash, &VERIFIER_INPUT_BLOCK_KEY, &password_parameters);
    password_encryptor.encrypted_verifier_input =
        encrypt_aes_cbc(&verifier, &verifier_key, &password_parameters.salt)?;
    let verifier_hash_key =
        derived_password_key(&base_hash, &VERIFIER_HASH_BLOCK_KEY, &password_parameters);
    password_encryptor.encrypted_verifier_hash = encrypt_aes_cbc(
        &zero_padded(password_parameters.hash.digest(&verifier))?,
        &verifier_hash_key,
        &password_parameters.salt,
    )?;
    let package_key_key =
        derived_password_key(&base_hash, &PACKAGE_KEY_BLOCK_KEY, &password_parameters);
    password_encryptor.encrypted_package_key =
        encrypt_aes_cbc(&package_key, &package_key_key, &password_parameters.salt)?;

    let mut encrypted_package = Vec::with_capacity(expected_ciphertext + 8);
    encrypted_package.extend_from_slice(&plaintext_len.to_le_bytes());
    for (segment, chunk) in plaintext_zip.chunks(PACKAGE_SEGMENT_BYTES).enumerate() {
        let segment = u32::try_from(segment).map_err(|_| OpcError::InvalidEncryptedPackage)?;
        let iv = initialization_vector(&key_data.salt, Some(&segment.to_le_bytes()), key_data.hash);
        encrypted_package.extend_from_slice(&encrypt_aes_cbc(
            &zero_padded(chunk.to_vec())?,
            &package_key,
            &iv,
        )?);
    }

    let mut mac = HmacState::new(key_data.hash, &hmac_key)?;
    mac.update(&encrypted_package);
    let hmac_value = mac.finalize();
    let integrity = DataIntegrity {
        encrypted_hmac_key: encrypt_aes_cbc(
            &hmac_key,
            &package_key,
            &initialization_vector(&key_data.salt, Some(&HMAC_KEY_BLOCK_KEY), key_data.hash),
        )?,
        encrypted_hmac_value: encrypt_aes_cbc(
            &zero_padded(hmac_value)?,
            &package_key,
            &initialization_vector(&key_data.salt, Some(&HMAC_VALUE_BLOCK_KEY), key_data.hash),
        )?,
    };
    let encryption_info = EncryptionDescriptor {
        key_data,
        integrity,
        password: password_encryptor,
    }
    .to_stream()?;

    build_compound_file(&encryption_info, &encrypted_package)
}

pub(crate) fn decrypt_package<R: Read + Seek>(
    reader: R,
    password: &str,
    limits: PackageReadLimits,
) -> Result<Vec<u8>> {
    let mut compound = cfb::CompoundFile::open(reader)?;
    let info_len = compound.entry("/EncryptionInfo")?.len();
    if info_len > MAX_ENCRYPTION_INFO_BYTES {
        return Err(OpcError::PackageLimitExceeded {
            kind: "encryption information size",
            limit: MAX_ENCRYPTION_INFO_BYTES,
        });
    }
    let mut info = Vec::with_capacity(usize::try_from(info_len).map_err(|_| {
        OpcError::PackageLimitExceeded {
            kind: "encryption information size",
            limit: MAX_ENCRYPTION_INFO_BYTES,
        }
    })?);
    compound
        .open_stream("/EncryptionInfo")?
        .read_to_end(&mut info)?;
    let descriptor = EncryptionDescriptor::parse(&info)?;
    let package_key = descriptor.authenticate_password(password)?;

    let encrypted_len = compound.entry("/EncryptedPackage")?.len();
    if encrypted_len < 8 {
        return Err(OpcError::InvalidEncryptedPackage);
    }
    let mut encrypted = compound.open_stream("/EncryptedPackage")?;
    descriptor.authenticate_stream(&package_key, &mut encrypted)?;
    encrypted.seek(SeekFrom::Start(0))?;
    decrypt_package_segments(
        &descriptor.key_data,
        &package_key,
        &mut encrypted,
        encrypted_len,
        limits,
    )
}

fn parse_descriptor_xml(xml: &[u8]) -> Result<EncryptionDescriptor> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut root_seen = false;
    let mut root_stage = 0_u8;
    let mut key_encryptor_uri = None;
    let mut key_data = None;
    let mut integrity = None;
    let mut password = None;

    loop {
        let (namespace, event) = reader.read_resolved_event_into(&mut buffer)?;
        let is_start = matches!(event, Event::Start(_));
        match &event {
            Event::Start(element) | Event::Empty(element) => {
                let local = element.local_name();
                let namespace = namespace_uri(&namespace)?;
                match depth {
                    0 if namespace == ENCRYPTION_NS && local.as_ref() == b"encryption" => {
                        if root_seen {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        root_seen = true;
                    }
                    1 if namespace == ENCRYPTION_NS && local.as_ref() == b"keyData" => {
                        if root_stage != 0 || is_start {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        key_data = Some(CipherParameters::from_element(element)?);
                        root_stage = 1;
                    }
                    1 if namespace == ENCRYPTION_NS && local.as_ref() == b"dataIntegrity" => {
                        if root_stage != 1 || is_start {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        integrity = Some(DataIntegrity {
                            encrypted_hmac_key: decode_attribute(element, b"encryptedHmacKey")?,
                            encrypted_hmac_value: decode_attribute(element, b"encryptedHmacValue")?,
                        });
                        root_stage = 2;
                    }
                    1 if namespace == ENCRYPTION_NS && local.as_ref() == b"keyEncryptors" => {
                        if root_stage != 2 || !is_start {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        root_stage = 3;
                    }
                    2 if namespace == ENCRYPTION_NS && local.as_ref() == b"keyEncryptor" => {
                        if key_encryptor_uri.is_some() || !is_start {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        let uri = required_attribute(element, b"uri")?;
                        if uri != PASSWORD_URI {
                            return Err(OpcError::UnsupportedEncryption(
                                "only password key encryptors are supported",
                            ));
                        }
                        key_encryptor_uri = Some(uri);
                    }
                    3 if namespace == PASSWORD_NS && local.as_ref() == b"encryptedKey" => {
                        if key_encryptor_uri.as_deref() != Some(PASSWORD_URI)
                            || password.is_some()
                            || is_start
                        {
                            return Err(OpcError::InvalidEncryptionInfo);
                        }
                        password = Some(PasswordKeyEncryptor::from_element(element)?);
                    }
                    _ => return Err(OpcError::InvalidEncryptionInfo),
                }
                if is_start {
                    depth = depth
                        .checked_add(1)
                        .ok_or(OpcError::InvalidEncryptionInfo)?;
                }
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(OpcError::InvalidEncryptionInfo)?;
            }
            Event::Text(text) if text.iter().all(u8::is_ascii_whitespace) => {}
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
            _ => return Err(OpcError::InvalidEncryptionInfo),
        }
        buffer.clear();
    }

    if !root_seen || depth != 0 || root_stage != 3 {
        return Err(OpcError::InvalidEncryptionInfo);
    }
    let key_data = key_data.ok_or(OpcError::InvalidEncryptionInfo)?;
    let integrity = integrity.ok_or(OpcError::InvalidEncryptionInfo)?;
    let password = password.ok_or(OpcError::InvalidEncryptionInfo)?;
    let encrypted_hmac_key_len = integrity.encrypted_hmac_key.len();
    if encrypted_hmac_key_len != round_up(key_data.salt.len(), 16)?
        && encrypted_hmac_key_len != round_up(key_data.hash.output_size(), 16)?
    {
        return Err(OpcError::InvalidEncryptionInfo);
    }
    validate_encrypted_field(
        &integrity.encrypted_hmac_value,
        round_up(key_data.hash.output_size(), 16)?,
    )?;
    validate_encrypted_field(
        &password.encrypted_package_key,
        round_up(key_data.key_bytes(), 16)?,
    )?;
    Ok(EncryptionDescriptor {
        key_data,
        integrity,
        password,
    })
}

fn namespace_uri<'a>(namespace: &'a ResolveResult<'a>) -> Result<&'a [u8]> {
    match namespace {
        ResolveResult::Bound(Namespace(uri)) => Ok(uri),
        ResolveResult::Unbound | ResolveResult::Unknown(_) => Err(OpcError::InvalidEncryptionInfo),
    }
}

fn required_attribute(element: &BytesStart<'_>, expected: &[u8]) -> Result<String> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        if attribute.key.as_ref() == expected {
            return Ok(attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                .into_owned());
        }
    }
    Err(OpcError::InvalidEncryptionInfo)
}

fn parse_usize_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<usize> {
    required_attribute(element, name)?
        .parse()
        .map_err(|_| OpcError::InvalidEncryptionInfo)
}

fn parse_u16_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<u16> {
    required_attribute(element, name)?
        .parse()
        .map_err(|_| OpcError::InvalidEncryptionInfo)
}

fn parse_u32_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<u32> {
    required_attribute(element, name)?
        .parse()
        .map_err(|_| OpcError::InvalidEncryptionInfo)
}

fn decode_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<Vec<u8>> {
    let value = required_attribute(element, name)?;
    BASE64_STANDARD
        .decode(value)
        .map_err(|_| OpcError::InvalidEncryptionInfo)
}

fn validate_encrypted_field(value: &[u8], expected_len: usize) -> Result<()> {
    if value.len() != expected_len || !value.len().is_multiple_of(16) {
        return Err(OpcError::InvalidEncryptionInfo);
    }
    Ok(())
}

fn round_up(value: usize, block_size: usize) -> Result<usize> {
    value
        .checked_add(block_size - 1)
        .map(|sum| sum / block_size * block_size)
        .ok_or(OpcError::InvalidEncryptionInfo)
}

fn password_hash(password: &str, encryptor: &PasswordKeyEncryptor) -> Result<Vec<u8>> {
    let mut utf16 = Vec::with_capacity(password.len().saturating_mul(2));
    for unit in password.encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    let mut input = Vec::with_capacity(encryptor.parameters.salt.len() + utf16.len());
    input.extend_from_slice(&encryptor.parameters.salt);
    input.extend_from_slice(&utf16);
    let mut hash = encryptor.parameters.hash.digest(&input);
    let mut iteration_input = Vec::with_capacity(4 + hash.len());
    for iteration in 0..encryptor.spin_count {
        iteration_input.clear();
        iteration_input.extend_from_slice(&iteration.to_le_bytes());
        iteration_input.extend_from_slice(&hash);
        hash = encryptor.parameters.hash.digest(&iteration_input);
    }
    Ok(hash)
}

fn derived_password_key(
    password_hash: &[u8],
    block_key: &[u8],
    parameters: &CipherParameters,
) -> Vec<u8> {
    let mut input = Vec::with_capacity(password_hash.len() + block_key.len());
    input.extend_from_slice(password_hash);
    input.extend_from_slice(block_key);
    let mut key = parameters.hash.digest(&input);
    key.resize(parameters.key_bytes(), 0x36);
    key.truncate(parameters.key_bytes());
    key
}

fn initialization_vector(salt: &[u8], block_key: Option<&[u8]>, hash: HashAlgorithm) -> Vec<u8> {
    let mut iv = if let Some(block_key) = block_key {
        let mut input = Vec::with_capacity(salt.len() + block_key.len());
        input.extend_from_slice(salt);
        input.extend_from_slice(block_key);
        hash.digest(&input)
    } else {
        salt.to_vec()
    };
    iv.resize(16, 0x36);
    iv.truncate(16);
    iv
}

fn zero_padded(mut value: Vec<u8>) -> Result<Vec<u8>> {
    value.resize(round_up(value.len(), 16)?, 0);
    Ok(value)
}

fn encrypt_aes_cbc(plaintext: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    if plaintext.is_empty() || !plaintext.len().is_multiple_of(16) || iv.len() != 16 {
        return Err(OpcError::InvalidEncryptionInfo);
    }
    let mut ciphertext = plaintext.to_vec();
    cbc::Encryptor::<Aes256>::new_from_slices(key, iv)
        .map_err(|_| OpcError::InvalidEncryptionInfo)?
        .encrypt_padded::<NoPadding>(&mut ciphertext, plaintext.len())
        .map_err(|_| OpcError::InvalidEncryptionInfo)?;
    Ok(ciphertext)
}

fn build_compound_file(encryption_info: &[u8], encrypted_package: &[u8]) -> Result<Vec<u8>> {
    let cursor = Cursor::new(Vec::new());
    let mut compound = cfb::CompoundFile::create_with_version(cfb::Version::V3, cursor)?;
    compound.create_storage_all(DATA_SPACES_STORAGE)?;
    compound.create_storage_all(format!("{DATA_SPACES_STORAGE}/DataSpaceInfo"))?;
    compound.create_storage_all(format!(
        "{DATA_SPACES_STORAGE}/TransformInfo/{STRONG_ENCRYPTION_TRANSFORM}"
    ))?;
    write_compound_stream(
        &mut compound,
        &format!("{DATA_SPACES_STORAGE}/Version"),
        &data_space_version_stream(),
    )?;
    write_compound_stream(
        &mut compound,
        &format!("{DATA_SPACES_STORAGE}/DataSpaceMap"),
        &data_space_map_stream(),
    )?;
    write_compound_stream(
        &mut compound,
        &format!("{DATA_SPACES_STORAGE}/DataSpaceInfo/{STRONG_ENCRYPTION_DATA_SPACE}"),
        &data_space_definition_stream(),
    )?;
    write_compound_stream(
        &mut compound,
        &format!("{DATA_SPACES_STORAGE}/TransformInfo/{STRONG_ENCRYPTION_TRANSFORM}/\u{6}Primary"),
        &transform_primary_stream(),
    )?;
    write_compound_stream(&mut compound, "/EncryptionInfo", encryption_info)?;
    write_compound_stream(&mut compound, "/EncryptedPackage", encrypted_package)?;
    compound.flush()?;
    Ok(compound.into_inner().into_inner())
}

fn write_compound_stream(
    compound: &mut cfb::CompoundFile<Cursor<Vec<u8>>>,
    path: &str,
    value: &[u8],
) -> Result<()> {
    compound.create_stream(path)?.write_all(value)?;
    Ok(())
}

fn data_space_version_stream() -> Vec<u8> {
    let mut stream = Vec::new();
    write_unicode_lp_p4(&mut stream, "Microsoft.Container.DataSpaces");
    write_version(&mut stream);
    write_version(&mut stream);
    write_version(&mut stream);
    stream
}

fn data_space_map_stream() -> Vec<u8> {
    let mut entry = Vec::new();
    entry.extend_from_slice(&1_u32.to_le_bytes());
    entry.extend_from_slice(&0_u32.to_le_bytes());
    write_unicode_lp_p4(&mut entry, "EncryptedPackage");
    write_unicode_lp_p4(&mut entry, STRONG_ENCRYPTION_DATA_SPACE);

    let mut stream = Vec::with_capacity(12 + entry.len());
    stream.extend_from_slice(&8_u32.to_le_bytes());
    stream.extend_from_slice(&1_u32.to_le_bytes());
    stream.extend_from_slice(
        &u32::try_from(entry.len() + 4)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    stream.extend_from_slice(&entry);
    stream
}

fn data_space_definition_stream() -> Vec<u8> {
    let mut stream = Vec::new();
    stream.extend_from_slice(&8_u32.to_le_bytes());
    stream.extend_from_slice(&1_u32.to_le_bytes());
    write_unicode_lp_p4(&mut stream, STRONG_ENCRYPTION_TRANSFORM);
    stream
}

fn transform_primary_stream() -> Vec<u8> {
    const TRANSFORM_ID: &str = "{FF9A3F03-56EF-4613-BDD5-5A41C1D07246}";
    const TRANSFORM_NAME: &str = "Microsoft.Container.EncryptionTransform";

    let mut stream = Vec::new();
    let transform_length = 12 + TRANSFORM_ID.encode_utf16().count() * 2;
    stream.extend_from_slice(
        &u32::try_from(transform_length)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    stream.extend_from_slice(&1_u32.to_le_bytes());
    write_unicode_lp_p4(&mut stream, TRANSFORM_ID);
    write_unicode_lp_p4(&mut stream, TRANSFORM_NAME);
    write_version(&mut stream);
    write_version(&mut stream);
    write_version(&mut stream);
    stream.extend_from_slice(&0_u32.to_le_bytes());
    stream.extend_from_slice(&0_u32.to_le_bytes());
    stream.extend_from_slice(&0_u32.to_le_bytes());
    stream.extend_from_slice(&4_u32.to_le_bytes());
    stream
}

fn write_unicode_lp_p4(output: &mut Vec<u8>, value: &str) {
    let encoded: Vec<u16> = value.encode_utf16().collect();
    let byte_len = encoded.len().saturating_mul(2);
    output.extend_from_slice(&u32::try_from(byte_len).unwrap_or(u32::MAX).to_le_bytes());
    for unit in encoded {
        output.extend_from_slice(&unit.to_le_bytes());
    }
    while !output.len().is_multiple_of(4) {
        output.push(0);
    }
}

fn write_version(output: &mut Vec<u8>) {
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&0_u16.to_le_bytes());
}

fn decrypt_aes_cbc(ciphertext: &[u8], key: &[u8], iv: &[u8]) -> Result<Vec<u8>> {
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) || iv.len() != 16 {
        return Err(OpcError::InvalidEncryptionInfo);
    }
    let mut plaintext = ciphertext.to_vec();
    macro_rules! decrypt {
        ($cipher:ty) => {
            cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                .map_err(|_| OpcError::InvalidEncryptionInfo)?
                .decrypt_padded::<NoPadding>(&mut plaintext)
                .map_err(|_| OpcError::InvalidEncryptionInfo)?
        };
    }
    match key.len() {
        16 => {
            decrypt!(Aes128);
        }
        24 => {
            decrypt!(Aes192);
        }
        32 => {
            decrypt!(Aes256);
        }
        _ => return Err(OpcError::InvalidEncryptionInfo),
    }
    Ok(plaintext)
}

fn decrypt_package_segments<R: Read>(
    parameters: &CipherParameters,
    package_key: &[u8],
    mut encrypted: R,
    encrypted_len: u64,
    limits: PackageReadLimits,
) -> Result<Vec<u8>> {
    let mut size_bytes = [0_u8; 8];
    read_exact_encrypted(&mut encrypted, &mut size_bytes)?;
    let plaintext_len_u64 = u64::from_le_bytes(size_bytes);
    if plaintext_len_u64 > limits.max_total_uncompressed_bytes {
        return Err(OpcError::PackageLimitExceeded {
            kind: "encrypted package plaintext size",
            limit: limits.max_total_uncompressed_bytes,
        });
    }
    let plaintext_len =
        usize::try_from(plaintext_len_u64).map_err(|_| OpcError::PackageLimitExceeded {
            kind: "encrypted package plaintext size",
            limit: usize::MAX as u64,
        })?;
    let expected_ciphertext = encrypted_package_ciphertext_len(plaintext_len)?;
    if encrypted_len != u64::try_from(expected_ciphertext.saturating_add(8)).unwrap_or(u64::MAX) {
        return Err(OpcError::InvalidEncryptedPackage);
    }

    let mut plaintext = Vec::with_capacity(plaintext_len);
    let mut plaintext_offset = 0_usize;
    let mut segment = 0_u32;
    while plaintext_offset < plaintext_len {
        let clear_len = (plaintext_len - plaintext_offset).min(PACKAGE_SEGMENT_BYTES);
        let encrypted_len = round_up(clear_len, 16)?;
        let mut ciphertext = vec![0_u8; encrypted_len];
        read_exact_encrypted(&mut encrypted, &mut ciphertext)?;
        let iv = initialization_vector(
            &parameters.salt,
            Some(&segment.to_le_bytes()),
            parameters.hash,
        );
        let clear = decrypt_aes_cbc(&ciphertext, package_key, &iv)?;
        plaintext.extend_from_slice(
            clear
                .get(..clear_len)
                .ok_or(OpcError::InvalidEncryptedPackage)?,
        );
        plaintext_offset += clear_len;
        segment = segment
            .checked_add(1)
            .ok_or(OpcError::InvalidEncryptedPackage)?;
    }
    Ok(plaintext)
}

fn read_exact_encrypted(reader: &mut impl Read, buffer: &mut [u8]) -> Result<()> {
    reader
        .read_exact(buffer)
        .map_err(|_| OpcError::InvalidEncryptedPackage)
}

fn encrypted_package_ciphertext_len(plaintext_len: usize) -> Result<usize> {
    let full_segments = plaintext_len / PACKAGE_SEGMENT_BYTES;
    let remainder = plaintext_len % PACKAGE_SEGMENT_BYTES;
    let full_len = full_segments
        .checked_mul(PACKAGE_SEGMENT_BYTES)
        .ok_or(OpcError::InvalidEncryptedPackage)?;
    if remainder == 0 {
        Ok(full_len)
    } else {
        full_len
            .checked_add(round_up(remainder, 16)?)
            .ok_or(OpcError::InvalidEncryptedPackage)
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
