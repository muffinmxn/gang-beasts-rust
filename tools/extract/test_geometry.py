import unittest
from types import SimpleNamespace
import numpy as np
from geometry import geometry, trs


def mesh():
    return SimpleNamespace(
        m_Vertices=[(100, 0, 0), (101, 0, 0), (100, 1, 0), (10, 0, 0), (12, 0, 0), (10, 3, 0)],
        m_Normals=[(0, 0, 1)]*6, m_UV0=[], m_UV1=[], m_Colors=[],
        m_Tangents=[],
        m_BoneWeights=[], m_BoneIndices=[],
        get_triangles=lambda: [[(0, 1, 2)], [(3, 4, 5)]],
    )


class GeometryTests(unittest.TestCase):
    def test_batch_selects_only_owned_vertices_and_removes_world_transform(self):
        world = trs([10, 0, 0], [0, 0, 0, 1], [2, 3, 1])
        attrs, groups = geometry(mesh(), [1], np.linalg.inv(world))
        np.testing.assert_allclose(attrs["POSITION"], [[0, 0, 0], [-1, 0, 0], [0, 1, 0]])
        self.assertEqual(groups, [[(0, 2, 1)]])
        # Reapplying the mirrored object's transform reproduces the original batch positions.
        mirrored_world = trs([-10, 0, 0], [0, 0, 0, 1], [2, 3, 1])
        restored = (mirrored_world @ np.column_stack((attrs["POSITION"], np.ones(3))).T).T[:, :3]
        np.testing.assert_allclose(restored, [[-10, 0, 0], [-12, 0, 0], [-10, 3, 0]])

    def test_negative_scale_keeps_winding_consistent(self):
        attrs, groups = geometry(mesh(), [1], np.diag([-1, 1, 1, 1]))
        self.assertEqual(groups, [[(0, 1, 2)]])
        self.assertEqual(attrs["NORMAL"][0], [0, 0, 1])

    def test_second_uv_set_is_preserved_for_baked_lighting(self):
        source = mesh()
        source.m_UV1 = [(0.1, 0.2), (0.2, 0.3), (0.3, 0.4),
                        (0.4, 0.5), (0.5, 0.6), (0.6, 0.7)]
        attrs, _ = geometry(source, [1])
        np.testing.assert_allclose(attrs["TEXCOORD_1"], [[0.4, 0.5], [0.5, 0.4], [0.6, 0.3]])

    def test_source_tangent_survives_x_mirror_and_v_flip(self):
        source = mesh()
        source.m_Tangents = [(1, 0, 0, -1)] * 6
        attrs, _ = geometry(source, [1])
        # X mirror changes tangent X. The coordinate reflection and exported UV-V
        # reflection cancel in the tangent-frame handedness, so W is preserved.
        np.testing.assert_allclose(attrs["TANGENT"], [[-1, 0, 0, -1]] * 3)

    def test_reflected_unbatch_transform_reverses_tangent_handedness(self):
        source = mesh()
        source.m_Tangents = [(1, 0, 0, 1)] * 6
        attrs, _ = geometry(source, [1], np.diag([-1, 1, 1, 1]))
        np.testing.assert_allclose(attrs["TANGENT"], [[1, 0, 0, -1]] * 3)

    def test_missing_material_uv0_defaults_to_origin_when_uv1_exists(self):
        source = mesh()
        source.m_UV1 = [(0.1, 0.2), (0.2, 0.3), (0.3, 0.4),
                        (0.4, 0.5), (0.5, 0.6), (0.6, 0.7)]
        attrs, _ = geometry(source, [1])
        np.testing.assert_array_equal(attrs["TEXCOORD_0"], [[0.0, 0.0]] * 3)

    def test_rigid_head_gets_implicit_unit_weights(self):
        attrs, _ = geometry(mesh(), [1], bind_count=1)
        self.assertEqual(attrs["JOINTS_0"], [[0, 0, 0, 0]]*3)
        self.assertEqual(attrs["WEIGHTS_0"], [[1, 0, 0, 0]]*3)

    def test_two_bone_influences_are_padded_to_gltf_vec4(self):
        source = mesh()
        source.m_BoneIndices = [(0, 1)] * 6
        source.m_BoneWeights = [(0.25, 0.75)] * 6
        attrs, _ = geometry(source, [1], bind_count=2)
        self.assertEqual(attrs["JOINTS_0"], [[1, 0, 0, 0]] * 3)
        np.testing.assert_allclose(attrs["WEIGHTS_0"], [[0.75, 0.25, 0, 0]] * 3)

    def test_missing_multibone_weights_fail_instead_of_exporting_broken_skin(self):
        with self.assertRaises(ValueError):
            geometry(mesh(), bind_count=2)

    def test_invalid_static_batch_range_fails(self):
        with self.assertRaises(ValueError):
            geometry(mesh(), [2])


if __name__ == "__main__":
    unittest.main()
