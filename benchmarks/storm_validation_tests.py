"""Bounded acquisition and SAC readiness controls; no network or real-data claims."""
import importlib.util
import io
import math
from pathlib import Path
import struct
import unittest
from unittest.mock import patch
import zipfile

ROOT=Path(__file__).resolve().parents[1]
SPEC=importlib.util.spec_from_file_location("storm",ROOT/"analysis/validate_storm_recording.py")
STORM=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(STORM)


def sac(endian="<"):
    floats=[-12345.0]*70
    floats[0]=.01;floats[5]=-.125;floats[6]=-.125+199*.01
    integers=[-12345]*40
    integers[:7]=[2024,225,8,38,15,0,6]
    integers[9]=200;integers[15]=1;integers[35]=1
    strings=b"203     "+b"-12345  "*23
    samples=[math.sin(i*.17) for i in range(200)]
    return struct.pack(endian+"70f",*floats)+struct.pack(endian+"40i",*integers)+strings+struct.pack(endian+"200f",*samples)


class SacControls(unittest.TestCase):
    def test_endianness_and_sample_zero_includes_negative_begin(self):
        for endian in ("<",">"):
            header,values=STORM.parse_sac(sac(endian))
            self.assertEqual(header["sample_count"],200)
            self.assertEqual(header["sample_zero_as_utc"],"2024-08-12T08:38:14.875000+00:00")
            self.assertEqual(len(values),200)
            self.assertIsNone(header["station_coordinate"]["latitude_deg"])
            self.assertIsNone(header["origin_offset_s"])

    def test_invalid_header_and_truncated_payload_fail(self):
        for offset,value in ((304,7),(316,201),(340,2),(420,0)):
            data=bytearray(sac());struct.pack_into("<i",data,offset,value)
            with self.subTest(offset=offset),self.assertRaises(ValueError):STORM.parse_sac(data)
        with self.assertRaises(ValueError):STORM.parse_sac(sac()[:-4])

    def test_nonfinite_samples_and_disagreeing_end_fail(self):
        for offset,value in ((632,float("nan")),(24,70.0)):
            data=bytearray(sac());struct.pack_into("<f",data,offset,value)
            with self.subTest(offset=offset),self.assertRaises(ValueError):STORM.parse_sac(data)

    def test_missing_position_is_not_zero_or_invented_truth(self):
        header,_=STORM.parse_sac(sac())
        self.assertEqual(header["station_coordinate"],{"latitude_deg":None,"longitude_deg":None,"height_m":None})
        self.assertEqual(header["source_coordinate"],{"latitude_deg":None,"longitude_deg":None,"height_m":None})

    def test_wav_preserves_count_rate_and_relative_signal(self):
        _,samples=STORM.parse_sac(sac())
        wav,gain,rate=STORM.wave_bytes(samples,100.000002235)
        self.assertEqual(rate,100)
        self.assertEqual(wav[:4],b"RIFF")
        self.assertEqual(struct.unpack_from("<I",wav,4)[0],len(wav)-8)
        self.assertEqual(struct.unpack_from("<H",wav,20)[0],3)
        position=wav.index(b"data")
        values=struct.unpack("<200f",wav[position+8:])
        self.assertAlmostEqual(max(abs(v) for v in values),.9,places=6)
        self.assertLess(max(abs(a-b/gain) for a,b in zip(samples,values)),1e-6)
        with self.assertRaises(ValueError):STORM.wave_bytes([0.0,0.0],100)


class ArchiveControls(unittest.TestCase):
    @staticmethod
    def archive(name="a/Infrasound.sac"):
        buffer=io.BytesIO()
        with zipfile.ZipFile(buffer,"w",zipfile.ZIP_DEFLATED) as file:file.writestr(name,sac())
        data=buffer.getvalue()
        start=data.index(b"PK\x01\x02");end=data.index(b"PK\x05\x06")
        entry=STORM.central_entries(data[start:end])[0]
        return data,entry

    def test_directory_and_member_crc(self):
        data,entry=self.archive()
        fields=struct.unpack("<4s5H3L2H",data[:30]);length=sum(fields[-2:])+entry["compressed_bytes"]
        self.assertEqual(STORM.inflate_member(entry,data[30:30+length],data[:30]),sac())
        entry["crc32"]^=1
        with self.assertRaisesRegex(ValueError,"CRC"):STORM.inflate_member(entry,data[30:30+length],data[:30])

    def test_unsafe_and_truncated_directory_rejected(self):
        for name in ("../escape.sac","/absolute.sac","C:drive.sac"):
            with self.subTest(name=name),self.assertRaises(ValueError):self.archive(name)
        data,_=self.archive()
        directory=data[data.index(b"PK\x01\x02"):data.index(b"PK\x05\x06")].replace(b"a/Infrasound.sac",b"a\\Infrasound.sac")
        with self.assertRaises(ValueError):STORM.central_entries(directory)
        with self.assertRaises(ValueError):STORM.central_entries(b"PK\x01\x02")

    def test_full_archive_response_refused_without_read(self):
        class Response:
            status=200;headers={}
            def __enter__(self):return self
            def __exit__(self,*args):pass
            def read(self,*args):raise AssertionError("Must not read an ignored range")
        with patch.object(STORM.urllib.request,"urlopen",return_value=Response()):
            with self.assertRaisesRegex(ValueError,"bounded range"):STORM.fetch_range(0,100)

    def test_range_bounds_rejected_before_network(self):
        with patch.object(STORM.urllib.request,"urlopen",side_effect=AssertionError("Unexpected network")):
            for start,length in ((-1,100),(0,0),(0,STORM.MAX_RANGE+1),(STORM.ARCHIVE_SIZE,1)):
                with self.subTest(start=start,length=length),self.assertRaises(ValueError):STORM.fetch_range(start,length)


if __name__=="__main__":unittest.main()
