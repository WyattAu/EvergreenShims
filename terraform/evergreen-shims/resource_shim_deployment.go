package main

import (
	"context"
	"time"

	"github.com/hashicorp/terraform-plugin-sdk/v2/diag"
	"github.com/hashicorp/terraform-plugin-sdk/v2/helper/schema"
)

func resourceShimDeployment() *schema.Resource {
	return &schema.Resource{
		CreateContext: resourceShimDeploymentCreate,
		ReadContext:   resourceShimDeploymentRead,
		UpdateContext: resourceShimDeploymentUpdate,
		DeleteContext: resourceShimDeploymentDelete,
		Importer: &schema.ResourceImporter{
			StateContext: schema.ImportStatePassthroughContext,
		},
		Timeouts: &schema.ResourceTimeout{
			Create: schema.DefaultTimeout(15 * time.Minute),
			Update: schema.DefaultTimeout(15 * time.Minute),
			Delete: schema.DefaultTimeout(15 * time.Minute),
		},
		Schema: map[string]*schema.Schema{
			"name": {
				Type:        schema.TypeString,
				Required:    true,
				ForceNew:    true,
				Description: "Name of the shim deployment.",
			},
			"namespace": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "default",
				Description: "Namespace for the deployment.",
			},
			"shim_config_name": {
				Type:        schema.TypeString,
				Required:    true,
				Description: "Reference to the ShimConfig to use.",
			},
			"replicas": {
				Type:        schema.TypeInt,
				Optional:    true,
				Default:     1,
				Description: "Number of replicas.",
			},
			"sidecar_enabled": {
				Type:        schema.TypeBool,
				Optional:    true,
				Default:     true,
				Description: "Enable sidecar injection.",
			},
			"management_api_port": {
				Type:        schema.TypeInt,
				Optional:    true,
				Default:     8080,
				Description: "Port for the management API.",
			},
			"health_check_path": {
				Type:        schema.TypeString,
				Optional:    true,
				Default:     "/healthz",
				Description: "Health check endpoint path.",
			},
			"selector_labels": {
				Type:     schema.TypeMap,
				Required: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Labels for pod selector.",
			},
			"annotations": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Annotations for the deployment.",
			},
			"labels": {
				Type:     schema.TypeMap,
				Optional: true,
				Elem:     &schema.Schema{Type: schema.TypeString},
				Description: "Labels for the deployment.",
			},
			"created_at": {
				Type:     schema.TypeString,
				Computed: true,
				Description: "Timestamp when the deployment was created.",
			},
			"status": {
				Type:     schema.TypeString,
				Computed: true,
				Description: "Current status of the deployment.",
			},
		},
	}
}

func resourceShimDeploymentCreate(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	name := d.Get("name").(string)
	d.SetId(name)
	return resourceShimDeploymentRead(ctx, d, meta)
}

func resourceShimDeploymentRead(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	if d.Id() == "" {
		return diag.Diagnostics{}
	}
	return nil
}

func resourceShimDeploymentUpdate(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	return resourceShimDeploymentRead(ctx, d, meta)
}

func resourceShimDeploymentDelete(ctx context.Context, d *schema.ResourceData, meta interface{}) diag.Diagnostics {
	d.SetId("")
	return nil
}
