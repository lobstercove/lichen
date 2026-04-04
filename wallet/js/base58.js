// Native Base58 implementation (Bitcoin alphabet, compatible with Rust bs58 crate)

const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

var bs58 = window.bs58 = {
    encode(buffer) {
        if (!buffer || buffer.length === 0) return '';

        const digits = [0];
        for (let index = 0; index < buffer.length; index++) {
            let carry = buffer[index];
            for (let digitIndex = 0; digitIndex < digits.length; digitIndex++) {
                carry += digits[digitIndex] << 8;
                digits[digitIndex] = carry % 58;
                carry = (carry / 58) | 0;
            }
            while (carry > 0) {
                digits.push(carry % 58);
                carry = (carry / 58) | 0;
            }
        }

        let output = '';
        for (let index = 0; buffer[index] === 0 && index < buffer.length - 1; index++) {
            output += BASE58_ALPHABET[0];
        }
        for (let index = digits.length - 1; index >= 0; index--) {
            output += BASE58_ALPHABET[digits[index]];
        }

        return output;
    },

    decode(string) {
        if (!string || string.length === 0) return new Uint8Array(0);

        const bytes = [0];
        for (let index = 0; index < string.length; index++) {
            const value = BASE58_ALPHABET.indexOf(string[index]);
            if (value === -1) {
                throw new Error(`Invalid base58 character: ${string[index]}`);
            }

            let carry = value;
            for (let byteIndex = 0; byteIndex < bytes.length; byteIndex++) {
                carry += bytes[byteIndex] * 58;
                bytes[byteIndex] = carry & 0xff;
                carry >>= 8;
            }
            while (carry > 0) {
                bytes.push(carry & 0xff);
                carry >>= 8;
            }
        }

        for (let index = 0; string[index] === BASE58_ALPHABET[0] && index < string.length - 1; index++) {
            bytes.push(0);
        }

        return new Uint8Array(bytes.reverse());
    }
};